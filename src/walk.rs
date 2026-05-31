//! Discover `.po` files under the configured roots via `ignore` + `globset`,
//! honoring `po_patterns` and `ignore_dirs`.
//!
//! Standard filters (`.gitignore`/hidden) are disabled for determinism — a
//! report tool should find the same files regardless of VCS state. Directory
//! pruning is driven solely by `ignore_dirs`; file selection by `po_patterns`
//! matched relative to each root.

use std::fmt;
use std::path::PathBuf;

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::config::Scan;

#[derive(Debug)]
pub enum WalkError {
    Glob(globset::Error),
    Walk(ignore::Error),
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Glob(error) => write!(f, "invalid po_patterns glob: {error}"),
            Self::Walk(error) => write!(f, "failed to walk directory: {error}"),
        }
    }
}

impl std::error::Error for WalkError {}

fn build_globset(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    builder.build()
}

/// Discover every `.po` file under `scan.roots`, sorted and deduplicated.
pub fn discover_po_files(scan: &Scan) -> Result<Vec<PathBuf>, WalkError> {
    let globset = build_globset(&scan.po_patterns).map_err(WalkError::Glob)?;
    let mut found = Vec::new();

    for root in &scan.roots {
        let ignore_dirs = scan.ignore_dirs.clone();
        let mut builder = WalkBuilder::new(root);
        builder.standard_filters(false);
        builder.filter_entry(move |entry| {
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            if !is_dir {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !ignore_dirs.iter().any(|dir| dir.as_str() == name)
        });

        for result in builder.build() {
            let entry = result.map_err(WalkError::Walk)?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            // Patterns are relative to the root (so `**/*.po` behaves the same
            // for `.`, an absolute path, or a nested root).
            let relative = path.strip_prefix(root).unwrap_or(path);
            if globset.is_match(relative) {
                found.push(path.to_path_buf());
            }
        }
    }

    found.sort();
    found.dedup();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(root: &std::path::Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
    }

    #[test]
    fn finds_nested_po_and_skips_ignored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(root, "top.po");
        write_file(root, "sub/deep/a.po");
        write_file(root, "vendor/v.po");
        write_file(root, "node_modules/n.po");
        write_file(root, ".git/g.po");
        write_file(root, "sub/notes.txt");

        let scan = Scan {
            roots: vec![root.to_path_buf()],
            ..Scan::default()
        };
        let found = discover_po_files(&scan).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(names.contains(&"top.po".to_string()));
        assert!(names.contains(&"sub/deep/a.po".to_string()));
        assert!(!names.iter().any(|n| n.contains("vendor")));
        assert!(!names.iter().any(|n| n.contains("node_modules")));
        assert!(!names.iter().any(|n| n.contains(".git")));
        assert!(!names.iter().any(|n| n.ends_with(".txt")));
    }
}
