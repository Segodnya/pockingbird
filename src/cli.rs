//! CLI surface (clap derive): `find <path> --config <path> --format text|json`.
//! Behind the `cli` feature. Exit code is always `0` (the tool reports, it does
//! not gate).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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

        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Text,
    Json,
}

/// Run the CLI. Returns the process exit code (always `0` in the MVP).
pub fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Find {
            path,
            config,
            format,
        } => {
            // Pipeline wired up in later phases. Phase 0: acknowledge the args.
            let _ = (path, config, format);
            0
        }
    }
}
