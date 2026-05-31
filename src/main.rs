//! Binary entrypoint (behind the `cli` feature). Parses args, runs the
//! pipeline, and exits with the returned code (always `0` in the MVP).

use clap::Parser;

use pockingbird::cli::{run, Cli};

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(cli));
}
