//! CLI surface (clap derive): `find <path> [knobs]` and `init [path]`. Behind
//! the `cli` feature. Exit code is always `0` (the tool reports, it does not
//! gate). This is the thin shell around the [`crate::scan`] facade: it resolves
//! config, renders progress to stderr, and writes the report to stdout — the
//! pipeline run itself is the testable core.
//!
//! Config resolution precedence (highest first): CLI flags → config file →
//! `[match].preset` → built-in defaults.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, Subcommand};

use crate::config::{Config, Format, MatchOverride, Preset, Tier};
use crate::pipeline::{PipelineError, PipelineEvent, ProgressSink};
use crate::{report, scan_with, Error};

/// A starter config that mirrors the built-in defaults (the `balanced` preset).
/// Written by `pockingbird init`. This *is* the repo's `pockingbird.toml`,
/// embedded at build time — the two can never drift.
pub const STARTER_CONFIG: &str = include_str!("../pockingbird.toml");

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

        /// Match preset baseline (replaces the config file's `[match]`).
        #[arg(long, value_enum)]
        preset: Option<Preset>,

        /// Minimum agreeing locales to report a group (overrides config).
        #[arg(long = "min-agree", value_name = "N")]
        min_agree: Option<usize>,

        /// Restrict to these match tiers (repeatable; overrides config `tiers`).
        #[arg(long = "tier", value_enum, value_name = "TIER")]
        tiers: Vec<Tier>,

        /// Locale ids to exclude (repeatable; overrides config `exclude`).
        #[arg(long = "exclude", value_name = "LOCALE")]
        exclude: Vec<String>,
    },

    /// Write a starter `pockingbird.toml` next to your catalogs.
    Init {
        /// Where to write the config (default `./pockingbird.toml`).
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,

        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
}

/// Resolved `find` inputs, bundled to keep the worker's signature small.
struct FindOptions {
    path: PathBuf,
    config_path: Option<PathBuf>,
    format: Option<Format>,
    preset: Option<Preset>,
    min_agree: Option<usize>,
    tiers: Vec<Tier>,
    exclude: Vec<String>,
}

/// Run the CLI. Always returns `0`: the tool reports, it never gates a build.
pub fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Find {
            path,
            config,
            format,
            preset,
            min_agree,
            tiers,
            exclude,
        } => find(FindOptions {
            path,
            config_path: config,
            format,
            preset,
            min_agree,
            tiers,
            exclude,
        }),
        Command::Init { path, force } => init(path, force),
    }
    0
}

fn find(opts: FindOptions) {
    use std::io::Write;

    let started = Instant::now();

    // CLI `[match]` knobs become an override layer; the resolver applies them on
    // top of the file (precedence: CLI > file > preset > default).
    let cli_match = MatchOverride {
        preset: opts.preset,
        tiers: (!opts.tiers.is_empty()).then_some(opts.tiers),
        min_locales_agree: opts.min_agree,
        ..MatchOverride::default()
    };
    let mut config = match load_config(opts.config_path.as_deref(), cli_match) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return;
        }
    };
    // `--exclude` is a flat list (no preset layering): set it directly.
    if !opts.exclude.is_empty() {
        config.locales.exclude = opts.exclude;
    }

    eprintln!("pockingbird: scanning {} …", opts.path.display());
    let mut progress = StderrProgress::default();
    // The facade is the single front door: it overrides roots from the positional
    // path, discovers, and runs the pipeline. The CLI is a thin shell over it.
    let report = match scan_with(&opts.path, &config, &mut progress) {
        Ok(report) => report,
        Err(error) => {
            render_scan_error(&error, &opts.path);
            return;
        }
    };

    let rendered = if wants_json(opts.format, &config) {
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

/// Write a starter config to `path` (default `pockingbird.toml`), refusing to
/// clobber an existing file unless `force`.
fn init(path: Option<PathBuf>, force: bool) {
    let path = path.unwrap_or_else(|| PathBuf::from("pockingbird.toml"));
    if path.exists() && !force {
        eprintln!(
            "pockingbird: {} already exists — pass --force to overwrite",
            path.display()
        );
        return;
    }
    match std::fs::write(&path, STARTER_CONFIG) {
        Ok(()) => eprintln!("pockingbird: wrote {}", path.display()),
        Err(error) => eprintln!("pockingbird: cannot write {}: {error}", path.display()),
    }
}

/// Render a [`scan_with`] failure to stderr, preserving the per-cause messages
/// the CLI showed back when it wired the pipeline itself.
fn render_scan_error(error: &Error, path: &Path) {
    match error {
        Error::Discover(error) => eprintln!("scan error: {error}"),
        Error::NoFiles => eprintln!("no .po files found under {}", path.display()),
        Error::Pipeline(PipelineError::NoCatalogs) => eprintln!("no catalogs parsed"),
        Error::Pipeline(PipelineError::Config(error)) => eprintln!("config error: {error}"),
    }
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
            PipelineEvent::FloorClamped {
                configured,
                effective,
            } => {
                eprintln!(
                    "pockingbird: min_locales_agree {configured} exceeds {effective} active \
                     locale(s) → lowered to {effective} (otherwise the report would be empty)"
                );
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

/// Load the config and resolve it against the CLI `[match]` override. With no
/// file, the override layers straight onto the built-in defaults.
fn load_config(path: Option<&Path>, cli_match: MatchOverride) -> Result<Config, String> {
    let text = match path {
        None => String::new(),
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read config {}: {error}", path.display()))?,
    };
    Config::from_toml_with(&text, cli_match).map_err(|error| match path {
        Some(path) => format!("invalid config {}: {error}", path.display()),
        None => format!("invalid config: {error}"),
    })
}

/// The `--format` flag wins; otherwise fall back to `[output].format`.
fn wants_json(format: Option<Format>, config: &Config) -> bool {
    matches!(format.unwrap_or(config.output.format), Format::Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a CLI `[match]` override the way `find` does from parsed flags.
    fn cli_override(
        preset: Option<Preset>,
        min_agree: Option<usize>,
        tiers: Vec<Tier>,
    ) -> MatchOverride {
        MatchOverride {
            preset,
            tiers: (!tiers.is_empty()).then_some(tiers),
            min_locales_agree: min_agree,
            ..MatchOverride::default()
        }
    }

    #[test]
    fn starter_config_parses_to_defaults() {
        // `init` must emit a config that round-trips to the built-in defaults.
        assert_eq!(
            Config::from_toml(STARTER_CONFIG).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn cli_overrides_follow_precedence() {
        // File sets a strict baseline; the CLI re-bases the preset to loose and
        // overrides min-agree + tiers. The loose fuzzy radius shows through where
        // nothing more specific set it.
        let cli = cli_override(Some(Preset::Loose), Some(2), vec![Tier::Exact]);
        let config = Config::from_toml_with("[match]\npreset = \"strict\"\n", cli).unwrap();
        assert_eq!(config.match_.fuzzy_max_distance, 3); // loose baseline (CLI preset)
        assert_eq!(config.match_.min_locales_agree, 2); // CLI override
        assert_eq!(config.match_.tiers, vec![Tier::Exact]); // CLI override
    }

    #[test]
    fn cli_preset_keeps_explicit_file_field() {
        // The decisive field-wise case: a CLI `--preset` re-bases the preset but
        // must NOT clobber a field the file set explicitly (CLI > file > preset).
        let cli = cli_override(Some(Preset::Loose), None, Vec::new());
        let config =
            Config::from_toml_with("[match]\npreset = \"strict\"\nmin_locales_agree = 3\n", cli)
                .unwrap();
        assert_eq!(config.match_.min_locales_agree, 3); // file explicit survives
        assert_eq!(config.match_.fuzzy_max_distance, 3); // loose baseline shows through
        assert_eq!(config.match_.tiers, Tier::ALL.to_vec()); // loose tiers (no override)
    }

    #[test]
    fn empty_cli_override_equals_file_only() {
        // No CLI knobs → identical to parsing the file alone.
        let text = "[match]\npreset = \"loose\"\n";
        assert_eq!(
            Config::from_toml_with(text, MatchOverride::default()).unwrap(),
            Config::from_toml(text).unwrap(),
        );
    }
}
