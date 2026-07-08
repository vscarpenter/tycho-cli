//! `tycho live` — the ratatui dashboard (spec §5.2).
//!
//! [`state`] is the pure, testable core: a [`state::DashboardState`] derived
//! from `(events, mtimes, now, tz, pricing)` with an injected `now` so
//! time-relative math (burn rate, active session) is deterministic in tests.
//! [`compute_snapshot`] wires it to the scan pipeline; the interactive event
//! loop lives in [`run`].

pub mod app;
pub mod state;
pub mod view;

use std::io;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::cost::{CostMode, Coster};
use crate::discover::{self, Provider, SearchRoot, TranscriptFile};
use crate::pricing::PricingTable;
use crate::scan::{self, EventFilter};
use app::{App, Control, handle_key};
use state::DashboardState;

/// How often the worker re-scans.
const REFRESH: Duration = Duration::from_secs(2);
/// How long the UI blocks waiting for a key before redrawing.
const POLL: Duration = Duration::from_millis(100);

/// Discover, scan, cost-stamp, and derive one dashboard snapshot. Does a
/// single discovery pass so file mtimes (for active-session detection) and
/// the parsed events come from the same file list. Applies the same rule as
/// [`scan::scan`]: `project` prefilters whole files only for Claude roots
/// (path-authoritative there); Codex and External files always parse, and
/// `project`/`model` are applied per event by [`scan::scan_files`] instead.
#[allow(clippy::too_many_arguments)]
pub fn compute_snapshot(
    roots: &[SearchRoot],
    project: Option<&str>,
    model: Option<&str>,
    provider: Option<Provider>,
    mode: CostMode,
    pricing: &PricingTable,
    tz: Tz,
    now: DateTime<Utc>,
) -> DashboardState {
    let files: Vec<TranscriptFile> = discover::discover(roots)
        .into_iter()
        .filter(|file| {
            file.provider != discover::Provider::Claude
                || project.is_none_or(|p| file.project.contains(p))
        })
        .collect();
    let mtimes = state::collect_mtimes(&files);
    let mut outcome = scan::scan_files(
        files,
        EventFilter {
            project,
            model,
            provider,
        },
    );
    Coster::new(pricing, mode).apply(&mut outcome.events);
    DashboardState::derive(outcome.events, &mtimes, now, tz, pricing)
}

/// Run the interactive dashboard until the user quits. Sets up the terminal
/// (raw mode, alternate screen, panic-restore hook) via `ratatui::init`, and
/// always restores it via `ratatui::restore`, even on error.
#[allow(clippy::too_many_arguments)]
pub fn run(
    roots: Vec<SearchRoot>,
    project: Option<String>,
    model: Option<String>,
    provider: Option<Provider>,
    mode: CostMode,
    pricing: PricingTable,
    tz: Tz,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(
        &mut terminal,
        roots,
        project,
        model,
        provider,
        mode,
        pricing,
        tz,
    );
    ratatui::restore();
    result
}

/// The draw/input/refresh loop. A worker thread re-scans every [`REFRESH`]
/// and streams snapshots over a channel; this thread only draws and reads
/// keys, so a slow scan never blocks input.
#[allow(clippy::too_many_arguments)]
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    roots: Vec<SearchRoot>,
    project: Option<String>,
    model: Option<String>,
    provider: Option<Provider>,
    mode: CostMode,
    pricing: PricingTable,
    tz: Tz,
) -> io::Result<()> {
    let (snap_tx, snap_rx) = mpsc::channel::<DashboardState>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    // The worker must own its inputs: thread::spawn requires a 'static
    // closure, so borrowing `roots`/`pricing` from this stack is impossible.
    let worker = thread::spawn(move || {
        loop {
            let state = compute_snapshot(
                &roots,
                project.as_deref(),
                model.as_deref(),
                provider,
                mode,
                &pricing,
                tz,
                Utc::now(),
            );
            if snap_tx.send(state).is_err() {
                break; // UI gone
            }
            match stop_rx.recv_timeout(REFRESH) {
                Err(RecvTimeoutError::Timeout) => continue,
                _ => break, // stop signal or disconnect
            }
        }
    });

    let mut app = App::default();
    let outcome = (|| -> io::Result<()> {
        loop {
            terminal.draw(|frame| view::draw(frame, &app))?;
            if event::poll(POLL)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && handle_key(key.code, key.modifiers, &mut app) == Control::Quit
            {
                break;
            }
            while let Ok(snap) = snap_rx.try_recv() {
                app.latest = Some(snap);
            }
        }
        Ok(())
    })();

    drop(stop_tx); // disconnect: worker exits at its next recv_timeout
    let _ = worker.join();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--project gsd` must include a Codex session whose path-derived
    /// project is a date shard (e.g. "2026"), not the cwd, because Codex
    /// carries its project in event metadata rather than the directory
    /// name. Mirrors `scan()`'s whole-file-skip-only-for-Claude rule
    /// (`src/scan.rs`); before this fix `compute_snapshot` pre-filtered
    /// every file by path project regardless of provider, so this Codex
    /// file was dropped entirely and contributed zero tokens.
    #[test]
    fn live_snapshot_matches_scan_for_codex_projects() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/07/08");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-a.jsonl"),
            [
                r#"{"type":"session_meta","timestamp":"2026-07-08T01:00:00Z","payload":{"id":"s","session_id":"s","cwd":"/Users/v/Projects/gsd"}}"#,
                r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:01Z","payload":{"model":"gpt-5.5","cwd":"/Users/v/Projects/gsd"}}"#,
                r#"{"type":"event_msg","timestamp":"2026-07-08T01:00:02Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":50}}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let roots = vec![SearchRoot {
            path: dir.path().to_path_buf(),
            provider: discover::Provider::Codex,
        }];
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-08T12:00:00Z".parse().unwrap();

        let snapshot = compute_snapshot(
            &roots,
            Some("gsd"),
            None,
            None,
            CostMode::Auto,
            &table,
            chrono_tz::UTC,
            now,
        );

        assert_eq!(
            snapshot.today.totals.total(),
            1050,
            "expected the Codex session's 1000 input + 50 output tokens for today"
        );
    }
}
