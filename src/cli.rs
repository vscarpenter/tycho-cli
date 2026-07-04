//! Command-line interface: argument definitions and their resolution.
//!
//! The derive structs define the grammar; `main.rs` owns execution. Usage
//! errors exit with code 2 (clap's default), runtime errors with 1.

use std::path::PathBuf;

use chrono::NaiveDate;
use chrono_tz::Tz;
use clap::{Args, Parser, Subcommand};

/// Top-level invocation. Running bare (`tycho`) is `tycho daily`.
#[derive(Debug, Parser)]
#[command(
    name = crate::BIN_NAME,
    version,
    about = "Usage analytics for Claude Code's local transcripts"
)]
pub struct Cli {
    /// The requested report; defaults to `daily`.
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Flags shared by every report.
    #[command(flatten)]
    pub global: GlobalArgs,
}

/// Available reports.
#[derive(Debug, Clone, Copy, Subcommand)]
pub enum Command {
    /// Per-day token usage with a totals row (the default command)
    Daily,
    /// Per-month token usage with a totals row
    Monthly,
    /// Per-session usage: start, duration, project, models, tokens
    Sessions {
        /// Show at most N sessions (totals still cover all matches)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Sort order, always descending
        #[arg(long, value_enum, default_value_t = SortKey::Start)]
        sort: SortKey,
    },
    /// Rollup by project directory
    Projects,
    /// Rollup by model
    Models,
    /// Cache economics: hit rate, actual vs no-cache cost, savings
    Cache,
    /// Data health: files, skipped lines, duplicates, date span
    Doctor,
    /// Live dashboard: today's usage, burn rate, and the active session
    Live,
    /// Per 5-hour billing block: tokens, cost, and the active block's projection
    Blocks,
}

/// `--sort` values for the sessions report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SortKey {
    /// Most recently started first
    Start,
    /// Largest total token count first
    Tokens,
    /// Longest wall-clock span first
    Duration,
}

impl From<SortKey> for crate::aggregate::SessionSort {
    fn from(key: SortKey) -> Self {
        match key {
            SortKey::Start => Self::Start,
            SortKey::Tokens => Self::Tokens,
            SortKey::Duration => Self::Duration,
        }
    }
}

/// Flags shared by every command. `global = true` lets them appear before
/// or after the subcommand.
#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Replace the default search roots (repeatable)
    #[arg(long = "dir", value_name = "PATH", global = true)]
    pub dirs: Vec<PathBuf>,

    /// Only include days on or after this date (YYYY-MM-DD, report timezone)
    #[arg(long, value_name = "DATE", global = true)]
    pub since: Option<NaiveDate>,

    /// Only include days on or before this date (YYYY-MM-DD, report timezone)
    #[arg(long, value_name = "DATE", global = true)]
    pub until: Option<NaiveDate>,

    /// Only include projects whose directory name contains this substring
    #[arg(long, value_name = "SUBSTR", global = true)]
    pub project: Option<String>,

    /// Only include models whose id contains this substring
    #[arg(long, value_name = "SUBSTR", global = true)]
    pub model: Option<String>,

    /// Report timezone as an IANA name, e.g. America/Chicago (default: system local)
    #[arg(long, value_name = "IANA", global = true, conflicts_with = "utc")]
    pub tz: Option<Tz>,

    /// Shorthand for --tz UTC
    #[arg(long, global = true)]
    pub utc: bool,

    /// Emit machine-readable JSON instead of a table
    #[arg(long, global = true)]
    pub json: bool,

    /// Emit CSV (daily, monthly, and sessions only)
    #[arg(long, global = true, conflicts_with = "json")]
    pub csv: bool,

    /// Cost mode: auto uses recorded costUSD when present, calculate
    /// always computes from tokens, display only sums recorded costUSD
    #[arg(long, value_enum, default_value_t = ModeArg::Auto, global = true)]
    pub mode: ModeArg,

    /// Extra pricing TOML merged over the built-in defaults
    #[arg(long, value_name = "PATH", global = true)]
    pub pricing: Option<PathBuf>,

    /// Show costs at 4 decimal places instead of 2
    #[arg(long, global = true)]
    pub precise: bool,
}

/// `--mode` values (see [`crate::cost::CostMode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ModeArg {
    /// Recorded costUSD when present, else calculated (the default)
    #[default]
    Auto,
    /// Always calculate from tokens and the pricing table
    Calculate,
    /// Only sum recorded costUSD values
    Display,
}

impl From<ModeArg> for crate::cost::CostMode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Auto => Self::Auto,
            ModeArg::Calculate => Self::Calculate,
            ModeArg::Display => Self::Display,
        }
    }
}

impl Cli {
    /// The command to run, applying the bare-invocation default.
    pub fn effective_command(&self) -> Command {
        self.command.unwrap_or(Command::Daily)
    }
}

/// Resolve the report timezone: `--utc` wins, then `--tz`, then the system
/// zone, falling back to UTC when the system zone cannot be determined.
pub fn resolve_timezone(global: &GlobalArgs) -> Tz {
    if global.utc {
        return chrono_tz::UTC;
    }
    if let Some(tz) = global.tz {
        return tz;
    }
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once(&"tycho").chain(args))
    }

    fn global(args: &[&str]) -> GlobalArgs {
        parse(args).unwrap().global
    }

    #[test]
    fn bare_invocation_defaults_to_daily() {
        let cli = parse(&[]).unwrap();
        assert!(matches!(cli.effective_command(), Command::Daily));
    }

    #[test]
    fn global_flags_work_before_and_after_the_subcommand() {
        for args in [
            &["--json", "daily", "--since", "2026-07-01"][..],
            &["daily", "--json", "--since", "2026-07-01"][..],
        ] {
            let cli = parse(args).unwrap();
            assert!(cli.global.json);
            assert_eq!(cli.global.since, NaiveDate::from_ymd_opt(2026, 7, 1));
        }
    }

    #[test]
    fn dir_is_repeatable() {
        let args = global(&["--dir", "/a", "--dir", "/b"]);
        assert_eq!(args.dirs, [PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn tz_and_utc_together_is_a_usage_error() {
        let err = parse(&["--tz", "America/Chicago", "--utc"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn invalid_date_and_timezone_are_usage_errors() {
        assert_eq!(
            parse(&["--since", "July 1st"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["--tz", "Mars/Olympus_Mons"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
    }

    #[test]
    fn resolve_timezone_prefers_utc_then_explicit_tz() {
        assert_eq!(resolve_timezone(&global(&["--utc"])), chrono_tz::UTC);
        assert_eq!(
            resolve_timezone(&global(&["--tz", "America/Chicago"])),
            chrono_tz::America::Chicago
        );
    }

    #[test]
    fn resolve_timezone_falls_back_to_a_real_zone_by_default() {
        // System zone on the test machine is unknown; the contract is only
        // that resolution succeeds and yields *some* zone.
        let _ = resolve_timezone(&global(&[]));
    }

    #[test]
    fn sessions_accepts_limit_and_sort() {
        let cli = parse(&["sessions", "--limit", "5", "--sort", "tokens"]).unwrap();
        match cli.effective_command() {
            Command::Sessions { limit, sort } => {
                assert_eq!(limit, Some(5));
                assert_eq!(sort, SortKey::Tokens);
            }
            other => panic!("expected sessions, got {other:?}"),
        }
    }

    #[test]
    fn invalid_sort_key_is_a_usage_error() {
        assert_eq!(
            parse(&["sessions", "--sort", "alphabetical"])
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidValue
        );
    }

    #[test]
    fn csv_conflicts_with_json() {
        assert_eq!(
            parse(&["daily", "--csv", "--json"]).unwrap_err().kind(),
            ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn every_new_subcommand_parses() {
        for cmd in ["monthly", "projects", "models", "doctor"] {
            assert!(parse(&[cmd]).is_ok(), "{cmd} should parse");
        }
    }
}
