//! CLI surface (clap derive): `find <path> --config <path> --format text|json`.
//! Behind the `cli` feature. Exit code is always `0` (the tool reports, it does
//! not gate). This is the thin shell around [`crate::pipeline`]: it discovers
//! files, renders progress to stderr, and writes the report to stdout — the
//! pipeline run itself is the testable core.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::{self, Config};
use crate::pipeline::{self, PipelineError, PipelineEvent, ProgressSink};
use crate::{po, report, walk};

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

    let mut progress = StderrProgress::default();
    let report = match pipeline::run(&files, po::parse_po, &config, &mut progress) {
        Ok(report) => report,
        Err(PipelineError::NoCatalogs) => {
            eprintln!("no catalogs parsed");
            return;
        }
        Err(PipelineError::Config(error)) => {
            eprintln!("config error: {error}");
            return;
        }
    };

    let rendered = if wants_json(format, &config) {
        report::to_json(&report.groups, report.total_keys)
    } else {
        report::to_text(&report.groups, report.total_keys)
    };
    eprintln!(
        "pockingbird: {} groups total in {:.1?}",
        report.groups.len(),
        started.elapsed()
    );
    // Ignore a broken pipe (e.g. piping into `head`) instead of panicking.
    let _ = writeln!(std::io::stdout(), "{rendered}");
}

/// Renders pipeline progress to stderr and times each tier. The presentation
/// (carriage-return counter, deferred skip list, per-tier elapsed) lives here,
/// off the core.
#[derive(Default)]
struct StderrProgress {
    skipped: Vec<String>,
    tier_started: Option<Instant>,
}

impl ProgressSink for StderrProgress {
    fn emit(&mut self, event: PipelineEvent<'_>) {
        match event {
            PipelineEvent::Skipped { path, error } => {
                self.skipped.push(format!("  {}: {error}", path.display()));
            }
            PipelineEvent::Parsing { done, total } => {
                eprint!("\rpockingbird: parsing {done}/{total}");
                if done == total {
                    eprintln!(); // close the progress line
                    if !self.skipped.is_empty() {
                        eprintln!("pockingbird: skipped {} file(s):", self.skipped.len());
                        for line in &self.skipped {
                            eprintln!("{line}");
                        }
                    }
                }
            }
            PipelineEvent::Matrix { locales, keys } => {
                eprintln!("pockingbird: matrix — {locales} locales × {keys} keys");
            }
            PipelineEvent::TierStarted(tier) => {
                eprint!("pockingbird: grouping {tier:?} …");
                self.tier_started = Some(Instant::now());
            }
            PipelineEvent::TierDone { groups, .. } => {
                let elapsed = self.tier_started.take().map(|t| t.elapsed());
                match elapsed {
                    Some(elapsed) => eprintln!(" {groups} groups ({elapsed:.1?})"),
                    None => eprintln!(" {groups} groups"),
                }
            }
        }
    }
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
