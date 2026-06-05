//! # pockingbird
//!
//! Report-only library + CLI that finds duplicate translation keys across
//! gettext `.po` catalogs.
//!
//! ## Library entry point
//!
//! The one call you need is [`scan`] — point it at a directory and a [`Config`]
//! and get back a [`Report`]:
//!
//! ```no_run
//! use std::path::Path;
//! use pockingbird::{scan, Config};
//!
//! let report = scan(Path::new("./locales"), &Config::default())?;
//! println!("{}", pockingbird::report::to_json(&report.groups, report.total_keys));
//! # Ok::<(), pockingbird::Error>(())
//! ```
//!
//! ## Pipeline
//!
//! ```text
//! walk → po → index → group → report
//! ```
//!
//! - `walk` — discover `.po` files under the configured roots.
//! - `po` — parse each catalog (polib) into keys and per-locale values;
//!   `locale` derives the locale id from the path.
//! - `index` — build the `KeyId × locale → Cell` matrix; `normalize` and
//!   `fuzzy` canonicalize cells per tier.
//! - `group` — tier-agnostic signature bucketing + leave-one-out over the
//!   canonical matrix → [`CandidateGroup`]s.
//! - [`report`] — render the groups as text (colored) or json.
//!
//! These stages are crate-internal; [`scan`] wires them together. [`config`]
//! holds the TOML schema and defaults that parameterize every stage.

use std::fmt;
use std::path::Path;

pub mod config;
pub mod report;

pub(crate) mod fuzzy;
pub(crate) mod group;
pub(crate) mod index;
pub(crate) mod locale;
pub(crate) mod normalize;
pub(crate) mod pipeline;
pub(crate) mod po;
pub(crate) mod walk;

#[cfg(feature = "cli")]
pub mod cli;

// Curated public surface: the result types a `scan` caller inspects, plus the
// config root. The mechanics that produce them stay crate-internal.
pub use config::Config;
pub use group::CandidateGroup;
pub use index::{Cell, KeyId};
pub use pipeline::{PipelineError, PipelineEvent, ProgressSink, Report, Skip};
pub use po::PoError;
pub use walk::WalkError;

/// A failed [`scan`]: discovery, no input, or a pipeline error.
#[derive(Debug)]
pub enum Error {
    /// Discovering `.po` files failed (bad glob, unreadable directory).
    Discover(WalkError),
    /// No `.po` files were found under the root.
    NoFiles,
    /// The pipeline could not produce a report (see [`PipelineError`]).
    Pipeline(PipelineError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discover(error) => write!(f, "{error}"),
            Self::NoFiles => write!(f, "no .po files found"),
            Self::Pipeline(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Scan a directory tree for duplicate translation keys across its `.po`
/// catalogs — the one-call library entry point. Discovers files under `root`
/// (overriding `config.scan.roots`), parses them, and runs the pipeline.
pub fn scan(root: &Path, config: &Config) -> Result<Report, Error> {
    scan_with(root, config, &mut ())
}

/// Like [`scan`], but drives a [`ProgressSink`] so callers can render progress.
pub fn scan_with<S>(root: &Path, config: &Config, sink: &mut S) -> Result<Report, Error>
where
    S: ProgressSink + ?Sized,
{
    let mut config = config.clone();
    config.scan.roots = vec![root.to_path_buf()];
    let files = walk::discover_po_files(&config.scan).map_err(Error::Discover)?;
    if files.is_empty() {
        return Err(Error::NoFiles);
    }
    pipeline::run(&files, po::parse_po, &config, sink).map_err(Error::Pipeline)
}
