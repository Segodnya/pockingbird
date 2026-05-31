//! CLI surface (clap derive): `find <path> --config <path> --format text|json`.
//! Behind the `cli` feature. Exit code is always `0` (the tool reports, it does
//! not gate). `run` wires the whole pipeline: `walk → po → index → group →
//! report`.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::{self, Config};
use crate::index::{build_matrix, CatalogInput};
use crate::{group, locale, po, report, walk};

#[derive(Debug, Parser)]
#[command(name = "pockingbird", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan a path for duplicate translation keys across `.po` catalogs.
    Find {
        /// Root path to scan for `.po` files.
        path: PathBuf,

        /// Path to a `pockingbird.toml` config file.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,

        /// Output format (overrides `[output].format`; default `text`).
        #[arg(long, value_enum)]
        format: Option<Format>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Text,
    Json,
}

/// Run the CLI. Always returns `0`: the tool reports, it never gates a build.
pub fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Find {
            path,
            config,
            format,
        } => find(path, config, format),
    }
    0
}

fn find(path: PathBuf, config_path: Option<PathBuf>, format: Option<Format>) {
    use std::io::Write;
    use std::time::Instant;

    let started = Instant::now();

    let mut config = match load_config(config_path.as_deref()) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return;
        }
    };
    // The positional path overrides the configured roots.
    config.scan.roots = vec![path.clone()];

    eprintln!("pockingbird: scanning {} …", path.display());
    let files = match walk::discover_po_files(&config.scan) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("scan error: {error}");
            return;
        }
    };
    if files.is_empty() {
        eprintln!("no .po files found under {}", path.display());
        return;
    }
    eprintln!("pockingbird: found {} .po files", files.len());

    let mut inputs = Vec::new();
    let mut skipped = Vec::new();
    let total = files.len();
    for (index, file) in files.iter().enumerate() {
        eprint!("\rpockingbird: parsing {}/{total}", index + 1);
        match po::parse_po(file) {
            Ok(catalog) => inputs.push(CatalogInput {
                locale: locale::locale_id(file),
                catalog,
            }),
            // Report-only: a malformed catalog is skipped with a warning, never
            // fatal (architecture rule: the client decides what to do).
            Err(error) => skipped.push(format!("  {}: {error}", file.display())),
        }
    }
    eprintln!(); // close the progress line
    if !skipped.is_empty() {
        eprintln!("pockingbird: skipped {} file(s):", skipped.len());
        for line in &skipped {
            eprintln!("{line}");
        }
    }
    if inputs.is_empty() {
        eprintln!("no catalogs parsed");
        return;
    }

    let mut matrix = build_matrix(&inputs, &config.locales.exclude);
    let total_keys = matrix.rows.len();
    eprintln!(
        "pockingbird: matrix — {} locales × {total_keys} keys",
        matrix.locales.len()
    );

    if let Err(error) = config.validate(matrix.locales.len()) {
        eprintln!("config error: {error}");
        return;
    }

    matrix.retain_eligible(config.match_.min_locales_agree);

    let mut groups = Vec::new();
    for &tier in &config.match_.tiers {
        let tier_started = Instant::now();
        eprint!("pockingbird: grouping {tier:?} …");
        let tier_groups = group::group_tier(&matrix, tier, &config.match_);
        eprintln!(
            " {} groups ({:.1?})",
            tier_groups.len(),
            tier_started.elapsed()
        );
        groups.extend(tier_groups);
    }

    let rendered = if wants_json(format, &config) {
        report::to_json(&groups, total_keys)
    } else {
        report::to_text(&groups, total_keys)
    };
    eprintln!(
        "pockingbird: {} groups total in {:.1?}",
        groups.len(),
        started.elapsed()
    );
    // Ignore a broken pipe (e.g. piping into `head`) instead of panicking.
    let _ = writeln!(std::io::stdout(), "{rendered}");
}

fn load_config(path: Option<&Path>) -> Result<Config, String> {
    match path {
        None => Ok(Config::default()),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read config {}: {error}", path.display()))?;
            Config::from_toml(&text)
                .map_err(|error| format!("invalid config {}: {error}", path.display()))
        }
    }
}

/// The `--format` flag wins; otherwise fall back to `[output].format`.
fn wants_json(format: Option<Format>, config: &Config) -> bool {
    match format {
        Some(Format::Json) => true,
        Some(Format::Text) => false,
        None => matches!(config.output.format, config::Format::Json),
    }
}
