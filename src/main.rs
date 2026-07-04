//! Thin CLI shell. All logic lives in the `tycho` library crate; this file
//! owns process concerns only: argument parsing, environment access, and
//! exit codes (0 success, 1 runtime error, 2 usage error).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use chrono_tz::Tz;
use clap::{CommandFactory, Parser};

use tycho::cli::{self, Cli, Command};
use tycho::report::{csv, json, table};
use tycho::scan::{self, EventFilter, ScanOutcome};
use tycho::{aggregate, discover};

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
    let roots = resolve_roots(&cli)?;
    let filter = EventFilter {
        project: cli.global.project.as_deref(),
        model: cli.global.model.as_deref(),
    };
    let outcome = scan::scan(&roots, filter);
    println!("{}", render(&cli, tz, &roots, outcome));
    Ok(())
}

fn resolve_roots(cli: &Cli) -> anyhow::Result<Vec<PathBuf>> {
    if !cli.global.dirs.is_empty() {
        return Ok(cli.global.dirs.clone());
    }
    let home = std::env::home_dir().context("cannot determine the home directory")?;
    let config_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
    Ok(discover::default_roots(&home, config_dir.as_deref()))
}

fn render(cli: &Cli, tz: Tz, roots: &[PathBuf], outcome: ScanOutcome) -> String {
    let global = &cli.global;
    let (since, until) = (global.since, global.until);
    match cli.effective_command() {
        Command::Daily => {
            let report = aggregate::daily(outcome.events, tz, since, until);
            if global.json {
                json::daily(&report, tz.name())
            } else if global.csv {
                csv::daily(&report)
            } else {
                table::daily(&report)
            }
        }
        Command::Monthly => {
            let report = aggregate::monthly(outcome.events, tz, since, until);
            if global.json {
                json::monthly(&report, tz.name())
            } else if global.csv {
                csv::monthly(&report)
            } else {
                table::monthly(&report)
            }
        }
        Command::Sessions { limit, sort } => {
            let report = aggregate::sessions(outcome.events, tz, since, until, sort.into(), limit);
            if global.json {
                json::sessions(&report, tz.name())
            } else if global.csv {
                csv::sessions(&report)
            } else {
                table::sessions(&report, tz)
            }
        }
        Command::Projects => {
            reject_csv(global.csv);
            let report = aggregate::projects(outcome.events, tz, since, until);
            if global.json {
                json::projects(&report, tz.name())
            } else {
                table::projects(&report, tz)
            }
        }
        Command::Models => {
            reject_csv(global.csv);
            let report = aggregate::models(outcome.events, tz, since, until);
            if global.json {
                json::models(&report, tz.name())
            } else {
                table::models(&report)
            }
        }
        Command::Doctor => {
            reject_csv(global.csv);
            let report = scan::doctor(roots, &outcome);
            if global.json {
                json::doctor(&report)
            } else {
                table::doctor(&report)
            }
        }
    }
}

/// `--csv` is a contract for daily/monthly/sessions only (spec §5); other
/// commands reject it as a usage error so scripts fail loudly, not oddly.
fn reject_csv(csv: bool) {
    if csv {
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "--csv is only supported for daily, monthly, and sessions; use --json",
            )
            .exit(); // exits with code 2
    }
}
