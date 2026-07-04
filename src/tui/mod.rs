//! `tycho live` — the ratatui dashboard (spec §5.2).
//!
//! [`state`] is the pure, testable core: a [`state::DashboardState`] derived
//! from `(events, mtimes, now, tz, pricing)` with an injected `now` so
//! time-relative math (burn rate, active session) is deterministic in tests.
//! [`compute_snapshot`] wires it to the scan pipeline; the interactive event
//! loop ([`run`]) arrives in the next task.

pub mod state;

use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::cost::{CostMode, Coster};
use crate::discover::{self, TranscriptFile};
use crate::pricing::PricingTable;
use crate::scan::{self, EventFilter};
use state::DashboardState;

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

/// Run the interactive dashboard (implemented in the next task).
pub fn run(
    _roots: Vec<PathBuf>,
    _project: Option<String>,
    _model: Option<String>,
    _mode: CostMode,
    _pricing: PricingTable,
    _tz: Tz,
) -> io::Result<()> {
    Ok(())
}
