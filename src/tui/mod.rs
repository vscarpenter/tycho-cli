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
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::cost::{CostMode, Coster};
use crate::discover::{self, TranscriptFile};
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
/// the parsed events come from the same file list. `project` filters whole
/// files up front; `model` filters per event during the scan.
pub fn compute_snapshot(
    roots: &[PathBuf],
    project: Option<&str>,
    model: Option<&str>,
    mode: CostMode,
    pricing: &PricingTable,
    tz: Tz,
    now: DateTime<Utc>,
) -> DashboardState {
    let files: Vec<TranscriptFile> = discover::discover(roots)
        .into_iter()
        .filter(|file| project.is_none_or(|p| file.project.contains(p)))
        .collect();
    let mtimes = state::collect_mtimes(&files);
    let mut outcome = scan::scan_files(
        files,
        EventFilter {
            project: None,
            model,
        },
    );
    Coster::new(pricing, mode).apply(&mut outcome.events);
    DashboardState::derive(outcome.events, &mtimes, now, tz, pricing)
}

/// Run the interactive dashboard until the user quits. Sets up the terminal
/// (raw mode, alternate screen, panic-restore hook) via `ratatui::init`, and
/// always restores it via `ratatui::restore`, even on error.
pub fn run(
    roots: Vec<PathBuf>,
    project: Option<String>,
    model: Option<String>,
    mode: CostMode,
    pricing: PricingTable,
    tz: Tz,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, roots, project, model, mode, pricing, tz);
    ratatui::restore();
    result
}

/// The draw/input/refresh loop. A worker thread re-scans every [`REFRESH`]
/// and streams snapshots over a channel; this thread only draws and reads
/// keys, so a slow scan never blocks input.
#[allow(clippy::too_many_arguments)]
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    roots: Vec<PathBuf>,
    project: Option<String>,
    model: Option<String>,
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
