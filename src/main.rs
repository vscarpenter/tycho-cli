//! Thin CLI shell. All logic lives in the `tycho` library crate; this file
//! owns process concerns only: argument parsing, environment access, and
//! exit codes (0 success, 1 runtime error, 2 usage error).

use std::process::ExitCode;

use anyhow::Context;
use clap::Parser;

use tycho::cli::{self, Cli, Command};
use tycho::scan::EventFilter;
use tycho::{aggregate, discover, report, scan};

fn main() -> ExitCode {
    let cli = Cli::parse(); // usage errors exit with code 2 here
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error:#}", tycho::BIN_NAME);
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let tz = cli::resolve_timezone(&cli.global);
    let roots = if cli.global.dirs.is_empty() {
        let home = std::env::home_dir().context("cannot determine the home directory")?;
        let config_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
        discover::default_roots(&home, config_dir.as_deref())
    } else {
        cli.global.dirs.clone()
    };
    let filter = EventFilter {
        project: cli.global.project.as_deref(),
        model: cli.global.model.as_deref(),
    };

    let outcome = scan::scan(&roots, filter);

    match cli.effective_command() {
        Command::Daily => {
            let report = aggregate::daily(outcome.events, tz, cli.global.since, cli.global.until);
            let rendered = if cli.global.json {
                report::json::daily(&report, tz.name())
            } else {
                report::table::daily(&report)
            };
            println!("{rendered}");
        }
    }
    Ok(())
}
