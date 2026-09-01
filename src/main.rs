//! Thin CLI shell. All logic lives in the `tycho` library crate; this file
//! owns process concerns only: argument parsing, environment access, and
//! exit codes (0 success, 1 runtime error, 2 usage error).

use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::Context;
use chrono::Utc;
use chrono_tz::Tz;
use clap::{CommandFactory, Parser};

use tycho::cli::{self, Cli, Command, ProviderArg};
use tycho::cost::{self, Coster};
use tycho::discover::{Provider, SearchRoot};
use tycho::pricing::{self, PricingTable};
use tycho::report::{csv, json, table};
use tycho::scan::{self, EventFilter, ScanOutcome};
use tycho::{aggregate, discover, tui};

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
    let pricing = resolve_pricing(&cli)?;

    if matches!(cli.effective_command(), Command::Live) {
        return run_live(&cli, tz, &roots, pricing);
    }

    let is_blocks = matches!(cli.effective_command(), Command::Blocks);
    let filter = EventFilter {
        project: cli.global.project.as_deref(),
        model: cli.global.model.as_deref(),
        provider: effective_provider(is_blocks, cli.global.provider),
    };

    let mut outcome = scan::scan(&roots, filter);

    let unpriced = cost::unknown_models(&outcome.events, &pricing);
    for model in &unpriced {
        eprintln!(
            "{}: no pricing for model {model:?}; costing it at $0 (add it via --pricing)",
            tycho::BIN_NAME
        );
    }
    let zero_rated = cost::zero_rated_models(&outcome.events, &pricing);
    let local = cost::local_models(&outcome.events, &pricing);
    let shadow = cost::shadow_diagnostic(&outcome.events, &pricing);
    Coster::new(&pricing, cli.global.mode.into()).apply(&mut outcome.events);

    println!(
        "{}",
        render(
            &cli, tz, &roots, outcome, unpriced, zero_rated, local, shadow, &pricing
        )
    );
    Ok(())
}

/// `tycho live`: an interactive dashboard on a TTY, or one JSON snapshot when
/// `--json` is set or stdout is not a terminal (so it stays scriptable and
/// never corrupts a redirected stream).
fn run_live(cli: &Cli, tz: Tz, roots: &[SearchRoot], pricing: PricingTable) -> anyhow::Result<()> {
    reject_csv(cli.global.csv);
    let g = &cli.global;
    let provider = effective_provider(false, g.provider);
    if g.json || !std::io::stdout().is_terminal() {
        let state = tui::compute_snapshot(
            roots,
            g.project.as_deref(),
            g.model.as_deref(),
            provider,
            g.mode.into(),
            &pricing,
            tz,
            Utc::now(),
        );
        println!("{}", json::live(&state, tz.name()));
        Ok(())
    } else {
        tui::run(
            roots.to_vec(),
            g.project.clone(),
            g.model.clone(),
            provider,
            g.mode.into(),
            pricing,
            tz,
        )?;
        Ok(())
    }
}

/// Blocks mirrors Claude's 5-hour usage-limit windows, so it defaults to
/// Claude events unless the user widens it with --provider. An explicit
/// `--provider all` maps to `None` (no filter), overriding that default.
fn effective_provider(command_is_blocks: bool, arg: Option<ProviderArg>) -> Option<Provider> {
    match (command_is_blocks, arg) {
        (_, Some(arg)) => cli::map_provider_arg(arg),
        (true, None) => Some(Provider::Claude),
        (false, None) => None,
    }
}

fn resolve_roots(cli: &Cli) -> anyhow::Result<Vec<SearchRoot>> {
    if !cli.global.dirs.is_empty() {
        return Ok(cli
            .global
            .dirs
            .iter()
            .cloned()
            .map(|path| discover::SearchRoot {
                path,
                provider: discover::Provider::External,
            })
            .collect());
    }
    let home = std::env::home_dir().context("cannot determine the home directory")?;
    let claude_config_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
    let codex_home = std::env::var("CODEX_HOME").ok();
    let pi_agent_dir = std::env::var("PI_CODING_AGENT_DIR").ok();
    Ok(discover::default_roots(
        &home,
        claude_config_dir.as_deref(),
        codex_home.as_deref(),
        pi_agent_dir.as_deref(),
    ))
}

/// Windows-side Claude Code roots that this process can see but is not
/// scanning. Only ever non-empty inside WSL, where `claude` run on Windows
/// and `claude` run in the distro keep separate transcript stores; the
/// Windows one shows up under the `/mnt/<drive>/Users` interop mount.
fn unscanned_windows_roots(roots: &[discover::SearchRoot]) -> Vec<std::path::PathBuf> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    let distro = std::env::var("WSL_DISTRO_NAME").ok();
    if !discover::is_wsl(osrelease.as_deref(), distro.as_deref()) {
        return Vec::new();
    }
    discover::windows_claude_roots(std::path::Path::new("/mnt"), roots)
}

/// Embedded defaults, then the user's config-dir table, then `--pricing`,
/// each merging per model over the previous layer.
fn resolve_pricing(cli: &Cli) -> anyhow::Result<PricingTable> {
    let mut table = PricingTable::embedded();
    if let Some(home) = std::env::home_dir() {
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let path = pricing::user_override_path(xdg.as_deref(), &home);
        if path.is_file() {
            let overrides =
                PricingTable::load(&path).with_context(|| format!("loading {}", path.display()))?;
            table.merge(overrides);
        }
    }
    if let Some(path) = &cli.global.pricing {
        let overrides =
            PricingTable::load(path).with_context(|| format!("loading {}", path.display()))?;
        table.merge(overrides);
    }
    // After both override layers: a user table may legitimately reference a
    // model defined only in the embedded defaults.
    table.validate()?;
    Ok(table)
}

#[allow(clippy::too_many_arguments)]
fn render(
    cli: &Cli,
    tz: Tz,
    roots: &[SearchRoot],
    outcome: ScanOutcome,
    unpriced: Vec<String>,
    zero_rated: Vec<String>,
    local: Vec<String>,
    shadow: cost::ShadowDiagnostic,
    pricing: &PricingTable,
) -> String {
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
                table::daily(&report, global.precise)
            }
        }
        Command::Monthly => {
            let report = aggregate::monthly(outcome.events, tz, since, until);
            if global.json {
                json::monthly(&report, tz.name())
            } else if global.csv {
                csv::monthly(&report)
            } else {
                table::monthly(&report, global.precise)
            }
        }
        Command::Sessions { limit, sort } => {
            let report = aggregate::sessions(outcome.events, tz, since, until, sort.into(), limit);
            if global.json {
                json::sessions(&report, tz.name())
            } else if global.csv {
                csv::sessions(&report)
            } else {
                table::sessions(&report, tz, global.precise)
            }
        }
        Command::Projects => {
            reject_csv(global.csv);
            let report = aggregate::projects(outcome.events, tz, since, until);
            if global.json {
                json::projects(&report, tz.name())
            } else {
                table::projects(&report, tz, global.precise)
            }
        }
        Command::Models => {
            reject_csv(global.csv);
            let report = aggregate::models(outcome.events, tz, since, until);
            if global.json {
                json::models(&report, tz.name())
            } else {
                table::models(&report, global.precise)
            }
        }
        Command::Cache => {
            reject_csv(global.csv);
            let report = tycho::cache::cache(outcome.events, tz, since, until, pricing);
            if global.json {
                json::cache(&report, tz.name())
            } else {
                table::cache(&report, global.precise)
            }
        }
        Command::Doctor => {
            reject_csv(global.csv);
            let report = scan::doctor(
                roots,
                &outcome,
                unpriced,
                zero_rated,
                local,
                shadow,
                unscanned_windows_roots(roots),
            );
            if global.json {
                json::doctor(&report)
            } else {
                table::doctor(&report)
            }
        }
        Command::Blocks => {
            reject_csv(global.csv);
            let report = tycho::blocks::blocks(outcome.events, tz, since, until, Utc::now());
            if global.json {
                json::blocks(&report, tz.name())
            } else {
                table::blocks(&report, tz, global.precise)
            }
        }
        Command::Live => unreachable!("live is handled in run_live before render"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_defaults_to_claude_provider() {
        assert_eq!(effective_provider(true, None), Some(Provider::Claude));
        assert_eq!(effective_provider(false, None), None);
        assert_eq!(
            effective_provider(true, Some(ProviderArg::Codex)),
            Some(Provider::Codex)
        );
        assert_eq!(
            effective_provider(false, Some(ProviderArg::Openai)),
            Some(Provider::External)
        );
        // Pi is not a 5-hour-block provider, so blocks keeps its Claude
        // default; --provider pi opts in explicitly.
        assert_eq!(
            effective_provider(true, Some(ProviderArg::Pi)),
            Some(Provider::Pi)
        );
    }

    #[test]
    fn provider_all_disables_the_filter_even_for_blocks() {
        // `all` means "no provider filter" — everything — and explicitly
        // overrides the Claude-only default that blocks would otherwise apply.
        assert_eq!(effective_provider(true, Some(ProviderArg::All)), None);
        assert_eq!(effective_provider(false, Some(ProviderArg::All)), None);
    }
}
