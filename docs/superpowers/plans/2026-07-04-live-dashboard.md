# `tycho live` Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `tycho live`, a ratatui dashboard that refreshes every 2 seconds showing today's totals/cost, a 10-minute burn rate, per-model split, a cache hit-rate gauge, and the active project (mtime).

**Architecture:** A background worker thread re-scans every 2 s and sends a fully-derived `DashboardState` over an mpsc channel; the UI thread only draws it and handles keys. `DashboardState::derive(events, mtimes, now, tz, pricing)` is pure (injected `now`) and reuses `cache::cache` + `aggregate::sessions`. Non-TTY / `--json` prints one snapshot and exits.

**Tech Stack:** Rust 2024, ratatui (+ `ratatui::crossterm` re-export), `std::sync::mpsc`, `std::thread`, `std::io::IsTerminal`, existing `scan`/`aggregate`/`cache`/`cost`/`pricing`.

## Global Constraints

- Rust stable, edition 2024, no `unsafe` (`unsafe_code = "forbid"`), no nightly.
- No `unwrap()`/`expect()` outside `#[cfg(test)]` (clippy `unwrap_used`/`expect_used` = deny).
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean at every commit.
- Library returns typed errors (`thiserror`); only `main.rs` uses `anyhow`. The TUI's only failure mode is terminal I/O, so `tui::run` returns `std::io::Result<()>`.
- `///` doc comments on every public item in the library.
- Commit style: Conventional Commits with scope; author `Vinny Carpenter <vscarpenter@gmail.com>`; last line `Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr`; no Co-Authored-By footer.
- Prefer iterators over index loops.

## Naming (locked; used across tasks)

- `scan::scan_files(files: Vec<TranscriptFile>, filter: EventFilter<'_>) -> ScanOutcome`
- `tui::state::DashboardState { today: CacheEconomics, burn: BurnRate, models: Vec<ModelLine>, sessions: Vec<SessionLine>, active: Option<ActiveSession>, generated_at: DateTime<Utc> }`
- `tui::state::BurnRate { tokens_per_min: f64, cost_per_hour: Decimal, per_minute: [u64; 10] }`
- `tui::state::ModelLine { model: String, tokens: u64, cost: Decimal, hit_rate: Option<Decimal> }`
- `tui::state::SessionLine { session_id: String, project: String, tokens: u64, cost: Decimal, last_activity: DateTime<Utc>, active: bool }`
- `tui::state::ActiveSession { project: String, idle: Duration }`  (chrono::Duration)
- `tui::state::DashboardState::derive(events: Vec<UsageEvent>, mtimes: &[(String, SystemTime)], now: DateTime<Utc>, tz: Tz, pricing: &PricingTable) -> DashboardState`
- `tui::state::burn_rate(events: &[UsageEvent], now: DateTime<Utc>) -> BurnRate`
- `tui::state::active_project(mtimes: &[(String, SystemTime)], now: DateTime<Utc>, threshold_secs: i64) -> Option<ActiveSession>`
- `tui::state::collect_mtimes(files: &[TranscriptFile]) -> Vec<(String, SystemTime)>`
- `tui::state::BURN_WINDOW_MINUTES: i64 = 10`, `tui::state::ACTIVE_THRESHOLD_SECS: i64 = 300`
- `tui::compute_snapshot(roots, project: Option<&str>, model: Option<&str>, mode: CostMode, pricing: &PricingTable, tz: Tz, now: DateTime<Utc>) -> DashboardState`
- `tui::run(roots: Vec<PathBuf>, project: Option<String>, model: Option<String>, mode: CostMode, pricing: PricingTable, tz: Tz) -> io::Result<()>`
- `tui::app::{App, Tab, Control, handle_key}` — `Tab { Overview, Sessions, Models }`, `Control { Continue, Quit }`
- `report::json::live(state: &DashboardState, timezone: &str) -> String` — top-level JSON key `"command": "live"`
- `cli::Command::Live`

---

## Task 1: `scan::scan_files` seam + ratatui dependency

**Files:**
- Modify: `Cargo.toml` (add `ratatui`)
- Modify: `src/scan.rs` (extract `scan_files`, redefine `scan`)
- Test: `src/scan.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `scan::scan_files(files: Vec<TranscriptFile>, filter: EventFilter<'_>) -> ScanOutcome`. `scan()` keeps its signature and behavior.

- [ ] **Step 1: Add the dependency.** In `Cargo.toml` under `[dependencies]`, after `rust_decimal`:

```toml
ratatui = "0.29"
```

Run `cargo add` is not needed; edit the file. Then `cargo build` to fetch.
Expected: compiles (nothing uses ratatui yet).

- [ ] **Step 2: Write the failing test** in `src/scan.rs` tests module:

```rust
#[test]
fn scan_files_parses_a_prediscovered_list() {
    let root = fixture_root();
    let files = crate::discover::discover(&[root.path().to_path_buf()]);
    let outcome = scan_files(files, EventFilter::default());
    assert_eq!(outcome.events.len(), 2);
    assert_eq!(outcome.summary.files_scanned, 3);
    assert_eq!(outcome.summary.duplicates_collapsed, 2);
}
```

- [ ] **Step 3: Run it, expect failure.** Run: `cargo test -p tycho-cli --lib scan_files_parses_a_prediscovered_list`
Expected: FAIL — `cannot find function scan_files`.

- [ ] **Step 4: Refactor `scan` into `scan` + `scan_files`.** Replace the current `pub fn scan(...)` body (lines ~52–99) with:

```rust
/// Scan all roots, honoring `filter`, and return deduplicated events.
///
/// Files parse in parallel (rayon); the merge stays sequential and in
/// discovery order, so results are deterministic regardless of thread
/// scheduling.
pub fn scan(roots: &[PathBuf], filter: EventFilter<'_>) -> ScanOutcome {
    let files: Vec<_> = discover::discover(roots)
        .into_iter()
        .filter(|file| {
            filter
                .project
                .is_none_or(|project| file.project.contains(project))
        })
        .collect();
    scan_files(files, filter)
}

/// Parse and deduplicate an already-discovered file list. The `project`
/// filter is assumed already applied to `files`; only the per-event `model`
/// filter is honored here. This is the seam `tycho live` uses so it can read
/// file mtimes from the same discovery pass (see `tui::compute_snapshot`).
pub fn scan_files(files: Vec<TranscriptFile>, filter: EventFilter<'_>) -> ScanOutcome {
    use rayon::prelude::*;

    let parsed: Vec<_> = files
        .into_par_iter()
        .map(|file| {
            let bytes = std::fs::metadata(&file.path).map(|m| m.len()).unwrap_or(0);
            let scan = record::parse_file(&file.path);
            (file, bytes, scan)
        })
        .collect();

    let mut summary = ScanSummary::default();
    let mut deduper = Deduper::new();
    for (file, bytes, scan) in parsed {
        let Ok(file_scan) = scan else {
            summary.files_unreadable += 1;
            continue;
        };
        summary.files_scanned += 1;
        summary.bytes_scanned += bytes;
        summary.stats.merge(&file_scan.stats);
        for mut event in file_scan.events {
            if let Some(model) = filter.model
                && !event.model.contains(model)
            {
                continue;
            }
            event.project = file.project.clone();
            deduper.insert(event);
        }
    }

    summary.duplicates_collapsed = deduper.collapsed();
    ScanOutcome {
        events: deduper.into_events().collect(),
        summary,
    }
}
```

Add the import `use crate::discover::{self, TranscriptFile};` — check the top of `scan.rs`: it currently has `use crate::discover;`. Change it to `use crate::discover::{self, TranscriptFile};`.

- [ ] **Step 5: Run the full scan test module, expect pass.** Run: `cargo test -p tycho-cli --lib scan`
Expected: PASS (new test + all existing `scan` tests unchanged).

- [ ] **Step 6: fmt + clippy.** Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit.**

```bash
git add Cargo.toml Cargo.lock src/scan.rs
git commit -F - <<'EOF'
refactor(scan): extract scan_files seam and add ratatui dep

scan_files parses an already-discovered file list so `tycho live` can read
mtimes from the same discovery pass instead of walking the tree twice.
scan() is now discover + project-filter + scan_files, behavior unchanged.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 2: `tui::state` core — module, types, `derive` (today, burn, models)

**Files:**
- Create: `src/tui/mod.rs` (just `pub mod state;` for now)
- Create: `src/tui/state.rs`
- Modify: `src/lib.rs` (add `pub mod tui;`)
- Test: `src/tui/state.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `scan::scan_files` (Task 1), `aggregate::{Totals, models, sessions, SessionSort}`, `cache::{cache, CacheEconomics}`, `pricing::PricingTable`, `record::UsageEvent`.
- Produces: `DashboardState`, `BurnRate`, `ModelLine`, `burn_rate`, and the constants. `sessions`/`active` land in Tasks 3–4 (start them as `Vec::new()` / `None`).

- [ ] **Step 1: Wire the module.** In `src/lib.rs` add after `pub mod scan;` (keep alphabetical-ish with the others): `pub mod tui;`. Create `src/tui/mod.rs`:

```rust
//! `tycho live` — the ratatui dashboard (spec §5.2).
//!
//! [`state`] is the pure, testable core: a [`state::DashboardState`] derived
//! from `(events, mtimes, now, tz, pricing)` with an injected `now` so
//! time-relative math (burn rate, active session) is deterministic in tests.
//! Rendering and the terminal event loop arrive in later tasks.

pub mod state;
```

- [ ] **Step 2: Write failing tests** in `src/tui/state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DedupKey, TokenUsage};
    use chrono::{DateTime, Utc};

    fn event(utc: &str, model: &str, usage: TokenUsage) -> UsageEvent {
        UsageEvent {
            timestamp: utc.parse::<DateTime<Utc>>().unwrap(),
            session_id: Some("s1".into()),
            project: "-Users-v-Projects-gsd".into(),
            model: model.into(),
            usage,
            cost_usd: None,
            cost: rust_decimal::Decimal::ZERO,
            dedup_key: DedupKey::Uuid(format!("{utc}-{model}-{}", usage.output)),
        }
    }

    const U: TokenUsage = TokenUsage {
        input: 100, output: 200, cache_write_5m: 50, cache_write_1h: 10, cache_read: 1_000,
    };

    fn priced(model: &str, utc: &str, usage: TokenUsage) -> UsageEvent {
        let table = PricingTable::embedded();
        let mut e = event(utc, model, usage);
        e.cost = crate::cost::Coster::new(&table, crate::cost::CostMode::Calculate).cost(&e);
        e
    }

    #[test]
    fn today_totals_cover_only_the_local_day_of_now() {
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let events = vec![
            priced("claude-opus-4-8", "2026-07-04T09:00:00Z", U), // today
            priced("claude-opus-4-8", "2026-07-03T09:00:00Z", U), // yesterday, excluded
        ];
        let state = DashboardState::derive(events, &[], now, chrono_tz::UTC, &table);
        assert_eq!(state.today.totals.output, 200);
        assert_eq!(state.today.totals.input, 100);
        // hit rate = 1000 / (100+50+10+1000)
        assert!(state.today.hit_rate.is_some());
    }

    #[test]
    fn models_line_carries_tokens_cost_and_hit_rate() {
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let events = vec![
            priced("claude-opus-4-8", "2026-07-04T09:00:00Z", U),
            priced("claude-sonnet-5", "2026-07-04T10:00:00Z", U),
        ];
        let state = DashboardState::derive(events, &[], now, chrono_tz::UTC, &table);
        assert_eq!(state.models.len(), 2);
        // largest total first; both have equal tokens so opus (higher cost) is not guaranteed,
        // but both must be present with a hit rate and non-zero cost.
        assert!(state.models.iter().all(|m| m.hit_rate.is_some()));
        assert!(state.models.iter().all(|m| m.cost > rust_decimal::Decimal::ZERO));
    }

    #[test]
    fn burn_rate_is_last_ten_minutes_bucketed_by_minute() {
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let tokens = TokenUsage { input: 60, output: 0, cache_write_5m: 0, cache_write_1h: 0, cache_read: 0 };
        let events = vec![
            event("2026-07-04T11:59:30Z", "m", tokens), // 0 min ago -> per_minute[9]
            event("2026-07-04T11:51:00Z", "m", tokens), // 9 min ago -> per_minute[0]
            event("2026-07-04T11:40:00Z", "m", tokens), // 20 min ago -> excluded
        ];
        let burn = burn_rate(&events, now);
        assert_eq!(burn.per_minute[9], 60);
        assert_eq!(burn.per_minute[0], 60);
        // 120 tokens in the window / 10 min
        assert!((burn.tokens_per_min - 12.0).abs() < 1e-9);
    }

    #[test]
    fn empty_events_yield_a_zero_state() {
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let state = DashboardState::derive(vec![], &[], now, chrono_tz::UTC, &table);
        assert_eq!(state.today.totals, crate::aggregate::Totals::default());
        assert_eq!(state.today.hit_rate, None);
        assert!(state.models.is_empty());
        assert_eq!(state.burn.per_minute, [0u64; 10]);
        assert_eq!(state.generated_at, now);
    }
}
```

- [ ] **Step 3: Run, expect failure.** Run: `cargo test -p tycho-cli --lib tui::state`
Expected: FAIL — types/functions undefined.

- [ ] **Step 4: Implement** the top of `src/tui/state.rs`:

```rust
//! The pure dashboard model: [`DashboardState`] and its derivation.
//!
//! Everything time-relative takes an injected `now`, never `Utc::now()`, so
//! the burn rate and active-session logic are deterministic under test.

use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;

use crate::aggregate::{self, SessionSort};
use crate::cache::{self, CacheEconomics};
use crate::pricing::PricingTable;
use crate::record::UsageEvent;

/// Width of the burn-rate window, in minutes (also the sparkline length).
pub const BURN_WINDOW_MINUTES: i64 = 10;

/// A transcript file touched within this many seconds of `now` marks its
/// project "active" (spec §5.2, "recent file mtime").
pub const ACTIVE_THRESHOLD_SECS: i64 = 300;

/// Tokens-per-minute burn over the last [`BURN_WINDOW_MINUTES`] minutes.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnRate {
    /// Total window tokens divided by the window width in minutes.
    pub tokens_per_min: f64,
    /// Window cost projected to an hourly run-rate.
    pub cost_per_hour: Decimal,
    /// Tokens per minute, oldest (index 0) to newest (index 9).
    pub per_minute: [u64; 10],
}

/// One row of the per-model split for today.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelLine {
    /// Model id.
    pub model: String,
    /// Total tokens across every category.
    pub tokens: u64,
    /// Actual cost (honors `--mode`).
    pub cost: Decimal,
    /// Cache hit rate, `None` when there was no cacheable input.
    pub hit_rate: Option<Decimal>,
}

impl From<&CacheEconomics> for ModelLine {
    fn from(row: &CacheEconomics) -> Self {
        Self {
            model: row.model.clone(),
            tokens: row.totals.total(),
            cost: row.actual_cost,
            hit_rate: row.hit_rate,
        }
    }
}

/// A live snapshot of usage, everything the dashboard renders.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardState {
    /// Today's totals plus cache economics (drives the headline + gauge).
    pub today: CacheEconomics,
    /// Burn rate over the last [`BURN_WINDOW_MINUTES`] minutes.
    pub burn: BurnRate,
    /// Per-model split for today, largest actual cost first.
    pub models: Vec<ModelLine>,
    /// Today's sessions, newest activity first (populated in Task 4).
    pub sessions: Vec<SessionLine>,
    /// The active session detected via file mtime (Task 3), if any.
    pub active: Option<ActiveSession>,
    /// When this snapshot was computed (the injected `now`).
    pub generated_at: DateTime<Utc>,
}

impl DashboardState {
    /// Derive a snapshot from cost-stamped `events`, file `mtimes`, and the
    /// current instant `now`. Today's aggregations use `now`'s local date in
    /// `tz`; the burn rate uses the absolute last-10-minute window.
    pub fn derive(
        events: Vec<UsageEvent>,
        mtimes: &[(String, SystemTime)],
        now: DateTime<Utc>,
        tz: Tz,
        pricing: &PricingTable,
    ) -> Self {
        let today_date = now.with_timezone(&tz).date_naive();
        let today_events: Vec<UsageEvent> = events
            .iter()
            .filter(|e| e.timestamp.with_timezone(&tz).date_naive() == today_date)
            .cloned()
            .collect();

        let cache_report = cache::cache(today_events.clone(), tz, None, None, pricing);
        let models = cache_report.models.iter().map(ModelLine::from).collect();

        let sessions = session_lines(today_events, tz);
        let active = active_project(mtimes, now, ACTIVE_THRESHOLD_SECS);
        let burn = burn_rate(&events, now);

        Self {
            today: cache_report.total,
            burn,
            models,
            sessions: mark_active(sessions, active.as_ref()),
            active,
            generated_at: now,
        }
    }
}

/// Sum every token category of one event.
fn event_tokens(event: &UsageEvent) -> u64 {
    let u = &event.usage;
    u.input + u.output + u.cache_write_5m + u.cache_write_1h + u.cache_read
}

/// Compute the burn rate over the last [`BURN_WINDOW_MINUTES`] minutes ending
/// at `now`. Events outside the window contribute nothing.
pub fn burn_rate(events: &[UsageEvent], now: DateTime<Utc>) -> BurnRate {
    let window_start = now - Duration::minutes(BURN_WINDOW_MINUTES);
    let mut per_minute = [0u64; 10];
    let mut window_tokens = 0u64;
    let mut window_cost = Decimal::ZERO;
    for event in events
        .iter()
        .filter(|e| e.timestamp > window_start && e.timestamp <= now)
    {
        let tokens = event_tokens(event);
        window_tokens += tokens;
        window_cost += event.cost;
        // 0 minutes ago is the newest bucket (index 9); 9 is the oldest.
        let mins_ago = now.signed_duration_since(event.timestamp).num_minutes();
        let idx = (9 - mins_ago).clamp(0, 9) as usize;
        per_minute[idx] += tokens;
    }
    BurnRate {
        tokens_per_min: window_tokens as f64 / BURN_WINDOW_MINUTES as f64,
        cost_per_hour: window_cost * Decimal::from(60 / BURN_WINDOW_MINUTES),
        per_minute,
    }
}
```

Because `sessions`/`active` are not implemented until Tasks 3–4, add **temporary stubs** at the bottom of the file (each replaced in its task) so this compiles:

```rust
/// One row of today's sessions (fully implemented in Task 4).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLine {
    /// Session id.
    pub session_id: String,
    /// Encoded project directory.
    pub project: String,
    /// Total tokens.
    pub tokens: u64,
    /// Actual cost.
    pub cost: Decimal,
    /// Latest event timestamp.
    pub last_activity: DateTime<Utc>,
    /// Whether this is the live session.
    pub active: bool,
}

/// The active session detected via file mtime (fully implemented in Task 3).
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSession {
    /// Encoded project directory of the most-recently-touched transcript.
    pub project: String,
    /// How long since that file was last written.
    pub idle: Duration,
}

fn session_lines(_events: Vec<UsageEvent>, _tz: Tz) -> Vec<SessionLine> {
    Vec::new()
}

fn mark_active(sessions: Vec<SessionLine>, _active: Option<&ActiveSession>) -> Vec<SessionLine> {
    sessions
}

/// Active-session detection stub (replaced in Task 3).
pub fn active_project(
    _mtimes: &[(String, SystemTime)],
    _now: DateTime<Utc>,
    _threshold_secs: i64,
) -> Option<ActiveSession> {
    None
}
```

Note: `SessionSort` import is unused until Task 4 — add `#[allow(unused_imports)]` on the `use crate::aggregate::{self, SessionSort};` line, OR import only `self` now and add `SessionSort` in Task 4. **Choose the latter**: change the import to `use crate::aggregate;` here and update it in Task 4.

- [ ] **Step 5: Run, expect pass.** Run: `cargo test -p tycho-cli --lib tui::state`
Expected: PASS (4 tests).

- [ ] **Step 6: fmt + clippy.** Run: `cargo fmt && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit.**

```bash
git add src/lib.rs src/tui/
git commit -F - <<'EOF'
feat(tui): DashboardState core with today totals, burn rate, models

Pure derivation from (events, mtimes, now, tz, pricing) with an injected now
for deterministic tests. Reuses cache::cache for today's totals + per-model
hit rates. Sessions and active-session detection are stubbed for the next
two tasks.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 3: active-session detection via mtime

**Files:**
- Modify: `src/tui/state.rs` (replace the `active_project` stub; add `collect_mtimes`)
- Test: `src/tui/state.rs`

**Interfaces:**
- Consumes: `discover::TranscriptFile`.
- Produces: real `active_project(...)` and `collect_mtimes(files: &[TranscriptFile]) -> Vec<(String, SystemTime)>`.

- [ ] **Step 1: Write failing tests** (add to the tests module):

```rust
fn secs_before(now: DateTime<Utc>, secs: i64) -> SystemTime {
    (now - Duration::seconds(secs)).into()
}

#[test]
fn active_project_picks_the_newest_within_threshold() {
    let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
    let mtimes = vec![
        ("proj-old".to_string(), secs_before(now, 200)),
        ("proj-new".to_string(), secs_before(now, 30)),
    ];
    let active = active_project(&mtimes, now, ACTIVE_THRESHOLD_SECS).unwrap();
    assert_eq!(active.project, "proj-new");
    assert_eq!(active.idle.num_seconds(), 30);
}

#[test]
fn active_project_is_none_when_all_files_are_stale() {
    let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
    let mtimes = vec![("proj".to_string(), secs_before(now, 600))];
    assert!(active_project(&mtimes, now, ACTIVE_THRESHOLD_SECS).is_none());
}

#[test]
fn active_project_is_none_with_no_files() {
    let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
    assert!(active_project(&[], now, ACTIVE_THRESHOLD_SECS).is_none());
}

#[test]
fn collect_mtimes_reads_each_files_project_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, "{}\n").unwrap();
    let files = vec![crate::discover::TranscriptFile {
        project: "-Users-v-Projects-gsd".into(),
        path,
    }];
    let mtimes = collect_mtimes(&files);
    assert_eq!(mtimes.len(), 1);
    assert_eq!(mtimes[0].0, "-Users-v-Projects-gsd");
}
```

- [ ] **Step 2: Run, expect failure.** Run: `cargo test -p tycho-cli --lib active_project`
Expected: FAIL (assertions fail — stub returns `None`; `collect_mtimes` undefined).

- [ ] **Step 3: Replace the `active_project` stub** and add `collect_mtimes`:

```rust
/// The active session: the most-recently-modified transcript file within
/// `threshold_secs` of `now`. mtime (not the last assistant event) is the
/// signal because non-assistant records touch the file too, so it reflects
/// live activity more sensitively (spec §5.2).
pub fn active_project(
    mtimes: &[(String, SystemTime)],
    now: DateTime<Utc>,
    threshold_secs: i64,
) -> Option<ActiveSession> {
    let (project, modified) = mtimes.iter().max_by_key(|(_, t)| *t)?;
    let idle = now.signed_duration_since(DateTime::<Utc>::from(*modified));
    (idle.num_seconds() <= threshold_secs).then(|| ActiveSession {
        project: project.clone(),
        idle: idle.max(Duration::zero()),
    })
}

/// Read each discovered file's mtime, dropping any that cannot be stat'd.
/// Split from [`active_project`] so the logic stays pure and testable.
pub fn collect_mtimes(files: &[crate::discover::TranscriptFile]) -> Vec<(String, SystemTime)> {
    files
        .iter()
        .filter_map(|file| {
            let modified = std::fs::metadata(&file.path).ok()?.modified().ok()?;
            Some((file.project.clone(), modified))
        })
        .collect()
}
```

- [ ] **Step 4: Run, expect pass.** Run: `cargo test -p tycho-cli --lib state`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/tui/state.rs
git commit -F - <<'EOF'
feat(tui): active-session detection from file mtime

active_project picks the newest transcript within a 5-minute threshold;
collect_mtimes does the stat I/O so the picker stays pure and unit-tested
with synthetic SystemTime tuples.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 4: session lines with the active flag

**Files:**
- Modify: `src/tui/state.rs` (replace `session_lines` + `mark_active` stubs; restore `SessionSort` import)
- Test: `src/tui/state.rs`

**Interfaces:**
- Consumes: `aggregate::{sessions, SessionSort, SessionTotals}`.
- Produces: real `session_lines`/`mark_active`; `DashboardState.sessions` populated.

- [ ] **Step 1: Write failing tests:**

```rust
#[test]
fn sessions_are_newest_activity_first_with_one_active() {
    let table = PricingTable::embedded();
    let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
    let mut older = priced("claude-opus-4-8", "2026-07-04T08:00:00Z", U);
    older.session_id = Some("old".into());
    let mut newer = priced("claude-opus-4-8", "2026-07-04T11:59:00Z", U);
    newer.session_id = Some("new".into());
    newer.project = "-Users-v-Projects-gsd".into();
    // one fresh file makes gsd active
    let mtimes = vec![("-Users-v-Projects-gsd".to_string(), (now - Duration::seconds(20)).into())];
    let state = DashboardState::derive(vec![older, newer], &mtimes, now, chrono_tz::UTC, &table);
    assert_eq!(state.sessions.len(), 2);
    assert_eq!(state.sessions[0].session_id, "new");
    assert!(state.sessions[0].active);
    assert!(!state.sessions[1].active);
}

#[test]
fn sessions_have_no_active_flag_when_nothing_is_live() {
    let table = PricingTable::embedded();
    let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
    let e = priced("claude-opus-4-8", "2026-07-04T08:00:00Z", U);
    let state = DashboardState::derive(vec![e], &[], now, chrono_tz::UTC, &table);
    assert_eq!(state.sessions.len(), 1);
    assert!(!state.sessions[0].active);
}
```

- [ ] **Step 2: Run, expect failure.** Run: `cargo test -p tycho-cli --lib sessions_are_newest`
Expected: FAIL (`state.sessions` empty).

- [ ] **Step 3: Restore the import.** Change `use crate::aggregate;` back to `use crate::aggregate::{self, SessionSort};`.

- [ ] **Step 4: Replace the `session_lines` and `mark_active` stubs:**

```rust
/// Roll today's events into session rows, newest activity first.
fn session_lines(events: Vec<UsageEvent>, tz: Tz) -> Vec<SessionLine> {
    let report = aggregate::sessions(events, tz, None, None, SessionSort::Start, None);
    let mut lines: Vec<SessionLine> = report
        .sessions
        .into_iter()
        .map(|s| SessionLine {
            session_id: s.session_id,
            project: s.project,
            tokens: s.totals.total(),
            cost: s.totals.cost,
            last_activity: s.end,
            active: false,
        })
        .collect();
    lines.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    lines
}

/// Flag the most-recently-active session when a live session was detected.
fn mark_active(mut sessions: Vec<SessionLine>, active: Option<&ActiveSession>) -> Vec<SessionLine> {
    if active.is_some()
        && let Some(first) = sessions.first_mut()
    {
        first.active = true;
    }
    sessions
}
```

- [ ] **Step 5: Run, expect pass.** Run: `cargo test -p tycho-cli --lib tui::state`
Expected: PASS (all state tests).

- [ ] **Step 6: fmt + clippy + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/tui/state.rs
git commit -F - <<'EOF'
feat(tui): today's session rows with the active flag

session_lines rolls today's events into rows sorted newest-activity-first;
mark_active flags the top row when active_project found a live session.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 5: `compute_snapshot`, `json::live`, and the `live` CLI command

**Files:**
- Modify: `src/tui/mod.rs` (add `compute_snapshot`)
- Modify: `src/report/json.rs` (add `live`)
- Modify: `src/cli.rs` (add `Command::Live`)
- Modify: `src/main.rs` (branch to a live path; JSON/non-TTY snapshot)
- Test: `src/report/json.rs`, `tests/cli.rs`

**Interfaces:**
- Consumes: `discover`, `scan::{scan_files, EventFilter}`, `cost::{Coster, CostMode}`, `state::DashboardState`.
- Produces: `tui::compute_snapshot(...)`, `json::live(...)`, `Command::Live`.

- [ ] **Step 1: `compute_snapshot` in `src/tui/mod.rs`.** Add imports and the function:

```rust
use std::path::PathBuf;
use std::time::SystemTime; // (only if needed; see note)

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::cost::{Coster, CostMode};
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
    let mut outcome = scan::scan_files(files, EventFilter { project: None, model });
    Coster::new(pricing, mode).apply(&mut outcome.events);
    DashboardState::derive(outcome.events, &mtimes, now, tz, pricing)
}
```

Remove the `use std::time::SystemTime;` line if the compiler flags it unused (it is only referenced through `state::collect_mtimes`). Keep the `pub mod state;` line at the top.

- [ ] **Step 2: Write the failing `json::live` test** in `src/report/json.rs` tests module:

```rust
#[test]
fn live_contract() {
    use crate::tui::state::{ActiveSession, BurnRate, DashboardState, ModelLine, SessionLine};
    let totals = Totals {
        input: 100, output: 200, cache_write_5m: 50, cache_write_1h: 10,
        cache_read: 1_000, cost: "0.64".parse().unwrap(),
    };
    let state = DashboardState {
        today: crate::cache::CacheEconomics {
            model: "Total".into(), totals,
            hit_rate: Some("0.86".parse().unwrap()),
            actual_cost: "0.64".parse().unwrap(),
            counterfactual_cost: "1.08".parse().unwrap(),
            savings: "0.44".parse().unwrap(),
            leverage: Some("1.68".parse().unwrap()),
        },
        burn: BurnRate { tokens_per_min: 12.0, cost_per_hour: "0.9".parse().unwrap(), per_minute: [1,2,3,4,5,6,7,8,9,10] },
        models: vec![ModelLine { model: "claude-opus-4-8".into(), tokens: 1_360, cost: "0.64".parse().unwrap(), hit_rate: Some("0.86".parse().unwrap()) }],
        sessions: vec![SessionLine { session_id: "s1".into(), project: "gsd".into(), tokens: 1_360, cost: "0.64".parse().unwrap(), last_activity: "2026-07-04T11:59:00Z".parse().unwrap(), active: true }],
        active: Some(ActiveSession { project: "gsd".into(), idle: chrono::Duration::seconds(20) }),
        generated_at: "2026-07-04T12:00:00Z".parse().unwrap(),
    };
    let value: serde_json::Value = serde_json::from_str(&live(&state, "UTC")).unwrap();
    assert_eq!(value["command"], "live");
    assert_eq!(value["generated_at"], "2026-07-04T12:00:00Z");
    assert_eq!(value["today"]["tokens"]["total"], 1_360);
    assert_eq!(value["today"]["hit_rate"], 0.86);
    assert_eq!(value["burn"]["tokens_per_min"], 12.0);
    assert_eq!(value["active"]["project"], "gsd");
    assert_eq!(value["models"][0]["model"], "claude-opus-4-8");
    assert_eq!(value["sessions"][0]["active"], true);
}
```

- [ ] **Step 3: Run, expect failure.** Run: `cargo test -p tycho-cli --lib live_contract`
Expected: FAIL — `live` undefined.

- [ ] **Step 4: Implement `json::live`.** Append to `src/report/json.rs` (before the tests module):

```rust
#[derive(Serialize)]
struct BurnOut {
    tokens_per_min: f64,
    cost_per_hour_usd: f64,
    per_minute: [u64; 10],
}

#[derive(Serialize)]
struct ModelLineOut {
    model: String,
    tokens: u64,
    cost_usd: f64,
    hit_rate: Option<f64>,
}

#[derive(Serialize)]
struct SessionLineOut {
    session_id: String,
    project: String,
    tokens: u64,
    cost_usd: f64,
    last_activity: String,
    active: bool,
}

#[derive(Serialize)]
struct ActiveOut {
    project: String,
    idle_seconds: i64,
}

#[derive(Serialize)]
struct LiveOut {
    command: &'static str,
    timezone: String,
    generated_at: String,
    today: CacheRowOut,
    burn: BurnOut,
    models: Vec<ModelLineOut>,
    sessions: Vec<SessionLineOut>,
    active: Option<ActiveOut>,
}

/// Render one live dashboard snapshot as pretty-printed JSON. This is what
/// `tycho live --json` (or `live` with a non-TTY stdout) prints.
pub fn live(state: &crate::tui::state::DashboardState, timezone: &str) -> String {
    use rust_decimal::prelude::ToPrimitive;
    let out = LiveOut {
        command: "live",
        timezone: timezone.to_owned(),
        generated_at: rfc3339(state.generated_at),
        today: CacheRowOut::from(&state.today),
        burn: BurnOut {
            tokens_per_min: state.burn.tokens_per_min,
            cost_per_hour_usd: state.burn.cost_per_hour.to_f64().unwrap_or(0.0),
            per_minute: state.burn.per_minute,
        },
        models: state
            .models
            .iter()
            .map(|m| ModelLineOut {
                model: m.model.clone(),
                tokens: m.tokens,
                cost_usd: m.cost.to_f64().unwrap_or(0.0),
                hit_rate: m.hit_rate.and_then(|r| r.to_f64()),
            })
            .collect(),
        sessions: state
            .sessions
            .iter()
            .map(|s| SessionLineOut {
                session_id: s.session_id.clone(),
                project: s.project.clone(),
                tokens: s.tokens,
                cost_usd: s.cost.to_f64().unwrap_or(0.0),
                last_activity: rfc3339(s.last_activity),
                active: s.active,
            })
            .collect(),
        active: state.active.as_ref().map(|a| ActiveOut {
            project: a.project.clone(),
            idle_seconds: a.idle.num_seconds(),
        }),
    };
    to_json(&out)
}
```

- [ ] **Step 5: Run, expect pass.** Run: `cargo test -p tycho-cli --lib live_contract`
Expected: PASS.

- [ ] **Step 6: Add `Command::Live` to `src/cli.rs`.** In the `Command` enum, after `Doctor`:

```rust
    /// Live dashboard: today's usage, burn rate, and the active session
    Live,
```

- [ ] **Step 7: Wire `main.rs`.** Add imports at the top: `use std::io::IsTerminal;` and `use chrono::Utc;`. In `run()`, right after `let filter = EventFilter { ... };` is built is too late (live shouldn't do the shared scan). Instead, insert a branch immediately after `let pricing = resolve_pricing(&cli)?;`:

```rust
    if matches!(cli.effective_command(), Command::Live) {
        return run_live(&cli, tz, &roots, pricing);
    }
```

Then add the function:

```rust
/// `tycho live`: an interactive dashboard on a TTY, or one JSON snapshot when
/// `--json` is set or stdout is not a terminal (so it stays scriptable and
/// never corrupts a redirected stream).
fn run_live(
    cli: &Cli,
    tz: Tz,
    roots: &[PathBuf],
    pricing: PricingTable,
) -> anyhow::Result<()> {
    reject_csv(cli.global.csv);
    let g = &cli.global;
    if g.json || !std::io::stdout().is_terminal() {
        let state = tycho::tui::compute_snapshot(
            roots,
            g.project.as_deref(),
            g.model.as_deref(),
            g.mode.into(),
            &pricing,
            tz,
            Utc::now(),
        );
        println!("{}", json::live(&state, tz.name()));
        Ok(())
    } else {
        tycho::tui::run(
            roots.to_vec(),
            g.project.clone(),
            g.model.clone(),
            g.mode.into(),
            pricing,
            tz,
        )?;
        Ok(())
    }
}
```

`tycho::tui::run` does not exist until Task 6. **To keep this task's commit compiling and testable, add a temporary shim** at the end of `src/tui/mod.rs`:

```rust
use std::io;

/// Run the interactive dashboard (implemented in Task 6).
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
```

Also update `render()`'s `match` so it stays exhaustive: add `Command::Live => unreachable!("live is handled in run_live before render"),`. (Clippy allows `unreachable!` — it is not a panic on malformed input, it is a structural invariant.) Alternatively return `String::new()`; prefer the `unreachable!` with the message.

- [ ] **Step 8: Write the integration test** in `tests/cli.rs`:

```rust
#[test]
fn live_json_emits_a_snapshot() {
    // Piped stdout is non-TTY, so bare `live` also emits JSON; assert both.
    for args in [&["live", "--json"][..], &["live"][..]] {
        let value = stdout_json(tycho().args(args));
        assert_eq!(value["command"], "live");
        assert!(value["generated_at"].is_string());
        assert!(value["today"]["tokens"]["total"].is_number());
        assert!(value["burn"]["per_minute"].as_array().unwrap().len() == 10);
        assert!(value["models"].is_array());
        assert!(value["sessions"].is_array());
    }
}
```

- [ ] **Step 9: Run the tests, expect pass.** Run: `cargo test -p tycho-cli live`
Expected: PASS (`live_contract` + `live_json_emits_a_snapshot`).

- [ ] **Step 10: fmt + clippy + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/tui/mod.rs src/report/json.rs src/cli.rs src/main.rs tests/cli.rs
git commit -F - <<'EOF'
feat(live): compute_snapshot, json::live, and the live command

Adds `tycho live`: a single discovery pass feeds both mtimes and the scan;
main.rs prints one JSON snapshot on --json or a non-TTY stdout (so it never
corrupts a pipe) and otherwise will launch the TUI. Interactive rendering is
stubbed for the next task.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 6: app state, key handling, rendering, and the threaded event loop

**Files:**
- Create: `src/tui/app.rs` (App, Tab, Control, handle_key)
- Create: `src/tui/view.rs` (rendering)
- Modify: `src/tui/mod.rs` (real `run`, worker loop; declare `app`/`view`; drop the shim)
- Test: `src/tui/app.rs`, `src/tui/view.rs`

**Interfaces:**
- Consumes: `state::DashboardState`, `ratatui`, `ratatui::crossterm`.
- Produces: `app::{App, Tab, Control, handle_key}`, `view::draw`, real `tui::run`.

- [ ] **Step 1: Create `src/tui/app.rs` with failing-first tests.** Write the file:

```rust
//! Dashboard UI state and key handling — the parts testable without a
//! terminal. Rendering lives in [`super::view`]; the event loop in
//! [`super::run`].

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::state::DashboardState;

/// Which tab is showing. `Tab` cycles with `Tab`/`BackTab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Headline totals, burn rate, cache gauge, active project.
    Overview,
    /// Today's sessions, active one first.
    Sessions,
    /// Today's per-model split.
    Models,
}

impl Tab {
    /// Tabs in display order.
    pub const ALL: [Tab; 3] = [Tab::Overview, Tab::Sessions, Tab::Models];

    /// Index into [`Tab::ALL`], for the ratatui `Tabs` widget.
    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// The next tab, wrapping.
    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    /// The previous tab, wrapping.
    pub fn prev(self) -> Tab {
        Tab::ALL[(self.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// Whether the event loop should keep running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Keep looping.
    Continue,
    /// Exit the dashboard.
    Quit,
}

/// Everything the UI thread holds between frames.
#[derive(Debug, Default)]
pub struct App {
    /// The current tab.
    pub tab: Tab,
    /// The latest snapshot, or `None` until the first scan lands.
    pub latest: Option<DashboardState>,
}

impl Default for Tab {
    fn default() -> Self {
        Tab::Overview
    }
}

/// Translate a keypress into a control-flow decision, mutating `app`.
/// `q`/`Esc`/`Ctrl-C` quit; `Tab`/`Right` and `BackTab`/`Left` cycle tabs.
pub fn handle_key(code: KeyCode, mods: KeyModifiers, app: &mut App) -> Control {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => Control::Quit,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => Control::Quit,
        KeyCode::Tab | KeyCode::Right => {
            app.tab = app.tab.next();
            Control::Continue
        }
        KeyCode::BackTab | KeyCode::Left => {
            app.tab = app.tab.prev();
            Control::Continue
        }
        _ => Control::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = App::default();
        assert_eq!(handle_key(KeyCode::Char('q'), KeyModifiers::NONE, &mut app), Control::Quit);
        assert_eq!(
            handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut app),
            Control::Quit
        );
        // plain 'c' does not quit
        assert_eq!(handle_key(KeyCode::Char('c'), KeyModifiers::NONE, &mut app), Control::Continue);
    }

    #[test]
    fn tab_cycles_forward_and_wraps() {
        let mut app = App::default();
        assert_eq!(app.tab, Tab::Overview);
        handle_key(KeyCode::Tab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Sessions);
        handle_key(KeyCode::Tab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Models);
        handle_key(KeyCode::Tab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Overview);
    }

    #[test]
    fn backtab_cycles_backward() {
        let mut app = App::default();
        handle_key(KeyCode::BackTab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Models);
    }
}
```

- [ ] **Step 2: Declare the modules** in `src/tui/mod.rs`: add `pub mod app;` and `pub mod view;` next to `pub mod state;`.

- [ ] **Step 3: Run app tests, expect pass.** Run: `cargo test -p tycho-cli --lib tui::app`
Expected: PASS (after `view.rs` exists — create an empty `src/tui/view.rs` with just the module doc first so the crate compiles; the render fn comes next step). Create `src/tui/view.rs`:

```rust
//! ratatui rendering of a [`DashboardState`] into a frame.
```

Then run the app tests. Expected: PASS.

- [ ] **Step 4: Implement `view::draw` with a render test.** Replace `src/tui/view.rs`:

```rust
//! ratatui rendering of a [`DashboardState`] into a frame, one panel per tab.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{
    Block, Borders, Cell, Gauge, Paragraph, Row, Sparkline, Table, Tabs,
};
use ratatui::Frame;
use rust_decimal::prelude::ToPrimitive;

use super::app::{App, Tab};
use super::state::DashboardState;

/// Draw the whole dashboard: a tab bar, the active panel, and a footer.
pub fn draw(frame: &mut Frame, app: &App) {
    let [tabs_area, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let titles = Tab::ALL.iter().map(|t| match t {
        Tab::Overview => "Overview",
        Tab::Sessions => "Sessions",
        Tab::Models => "Models",
    });
    frame.render_widget(
        Tabs::new(titles).select(app.tab.index()).highlight_style(
            Style::default().add_modifier(Modifier::REVERSED),
        ),
        tabs_area,
    );

    match &app.latest {
        None => frame.render_widget(Paragraph::new("Scanning…"), body),
        Some(state) => match app.tab {
            Tab::Overview => overview(frame, body, state),
            Tab::Sessions => sessions(frame, body, state),
            Tab::Models => models(frame, body, state),
        },
    }

    let footer_text = match &app.latest {
        Some(s) => format!(
            "updated {}  ·  q quit  ·  tab switch",
            s.generated_at.format("%H:%M:%S")
        ),
        None => "q quit  ·  tab switch".to_owned(),
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().dim()),
        footer,
    );
}

fn dollars(d: rust_decimal::Decimal) -> String {
    format!("${:.2}", d.to_f64().unwrap_or(0.0))
}

fn overview(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let [head, gauge_area, spark_area] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .areas(area);

    let active = state
        .active
        .as_ref()
        .map(|a| format!("{} (idle {}s)", a.project, a.idle.num_seconds()))
        .unwrap_or_else(|| "idle".to_owned());
    let lines = vec![
        Line::from(format!("Today   {}   {} tokens", dollars(state.today.actual_cost), state.today.totals.total())),
        Line::from(format!(
            "Burn    {:.0} tok/min   {}/hr",
            state.burn.tokens_per_min,
            dollars(state.burn.cost_per_hour)
        )),
        Line::from(format!("Active  {active}")),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("tycho live")),
        head,
    );

    let ratio = state.today.hit_rate.and_then(|r| r.to_f64()).unwrap_or(0.0).clamp(0.0, 1.0);
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Cache hit rate"))
            .ratio(ratio)
            .label(format!("{:.1}%", ratio * 100.0)),
        gauge_area,
    );

    frame.render_widget(
        Sparkline::default()
            .block(Block::default().borders(Borders::ALL).title("Burn (10 min)"))
            .data(&state.burn.per_minute),
        spark_area,
    );
}

fn models(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let rows = state.models.iter().map(|m| {
        Row::new(vec![
            Cell::from(m.model.clone()),
            Cell::from(m.tokens.to_string()),
            Cell::from(dollars(m.cost)),
            Cell::from(
                m.hit_rate
                    .and_then(|r| r.to_f64())
                    .map(|r| format!("{:.1}%", r * 100.0))
                    .unwrap_or_else(|| "-".into()),
            ),
        ])
    });
    let table = Table::new(
        rows,
        [Constraint::Min(20), Constraint::Length(12), Constraint::Length(10), Constraint::Length(8)],
    )
    .header(Row::new(vec!["Model", "Tokens", "Cost", "Hit"]).style(Style::default().bold()))
    .block(Block::default().borders(Borders::ALL).title("Models (today)"));
    frame.render_widget(table, area);
}

fn sessions(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let rows = state.sessions.iter().map(|s| {
        let marker = if s.active { "●" } else { " " };
        Row::new(vec![
            Cell::from(marker),
            Cell::from(s.session_id.chars().take(8).collect::<String>()),
            Cell::from(s.project.clone()),
            Cell::from(s.tokens.to_string()),
            Cell::from(dollars(s.cost)),
        ])
    });
    let table = Table::new(
        rows,
        [Constraint::Length(2), Constraint::Length(10), Constraint::Min(16), Constraint::Length(12), Constraint::Length(10)],
    )
    .header(Row::new(vec!["", "Session", "Project", "Tokens", "Cost"]).style(Style::default().bold()))
    .block(Block::default().borders(Borders::ALL).title("Sessions (today)"));
    frame.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sample() -> DashboardState {
        use crate::aggregate::Totals;
        use super::super::state::{BurnRate, ModelLine, SessionLine};
        let totals = Totals { input: 100, output: 200, cache_write_5m: 50, cache_write_1h: 10, cache_read: 1_000, cost: "0.64".parse().unwrap() };
        DashboardState {
            today: crate::cache::CacheEconomics {
                model: "Total".into(), totals, hit_rate: Some("0.86".parse().unwrap()),
                actual_cost: "0.64".parse().unwrap(), counterfactual_cost: "1.08".parse().unwrap(),
                savings: "0.44".parse().unwrap(), leverage: Some("1.68".parse().unwrap()),
            },
            burn: BurnRate { tokens_per_min: 12.0, cost_per_hour: "0.9".parse().unwrap(), per_minute: [1,2,3,4,5,6,7,8,9,10] },
            models: vec![ModelLine { model: "claude-opus-4-8".into(), tokens: 1_360, cost: "0.64".parse().unwrap(), hit_rate: Some("0.86".parse().unwrap()) }],
            sessions: vec![SessionLine { session_id: "sess-abcd".into(), project: "gsd".into(), tokens: 1_360, cost: "0.64".parse().unwrap(), last_activity: "2026-07-04T11:59:00Z".parse().unwrap(), active: true }],
            active: Some(super::super::state::ActiveSession { project: "gsd".into(), idle: chrono::Duration::seconds(20) }),
            generated_at: "2026-07-04T12:00:00Z".parse().unwrap(),
        }
    }

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn overview_shows_headline_and_active_project() {
        let text = render(&App { tab: Tab::Overview, latest: Some(sample()) });
        assert!(text.contains("Today"));
        assert!(text.contains("gsd"));
        assert!(text.contains("Cache hit rate"));
    }

    #[test]
    fn models_tab_lists_the_model() {
        let text = render(&App { tab: Tab::Models, latest: Some(sample()) });
        assert!(text.contains("claude-opus-4-8"));
    }

    #[test]
    fn no_snapshot_shows_scanning() {
        let text = render(&App::default());
        assert!(text.contains("Scanning"));
    }
}
```

Note on `.content()`: ratatui 0.29 `Buffer` exposes `content()`. If the compiler says it is private, use the public field `buffer.content.iter()` instead (drop the `()`).

- [ ] **Step 5: Run view tests, expect pass.** Run: `cargo test -p tycho-cli --lib tui::view`
Expected: PASS (3 tests).

- [ ] **Step 6: Replace the `run` shim with the real loop** in `src/tui/mod.rs`. Remove the temporary `run` shim; replace with:

```rust
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::{handle_key, App, Control};

/// How often the worker re-scans.
const REFRESH: Duration = Duration::from_secs(2);
/// How long the UI blocks waiting for a key before redrawing.
const POLL: Duration = Duration::from_millis(100);

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
            let state =
                compute_snapshot(&roots, project.as_deref(), model.as_deref(), mode, &pricing, tz, Utc::now());
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
            if event::poll(POLL)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press
                        && handle_key(key.code, key.modifiers, &mut app) == Control::Quit
                    {
                        break;
                    }
                }
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
```

- [ ] **Step 7: Full test + build.** Run: `cargo test -p tycho-cli` then `cargo build --release`
Expected: all tests PASS; release builds.

- [ ] **Step 8: fmt + clippy + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/tui/
git commit -F - <<'EOF'
feat(live): ratatui dashboard with three tabs and a scan thread

App/Tab/handle_key are terminal-free and unit-tested; view::draw renders the
Overview (headline, cache gauge, burn sparkline), Sessions, and Models tabs,
verified against a TestBackend. run() spawns a worker that re-scans every 2s
and streams snapshots over an mpsc channel while the UI stays responsive;
ratatui::init/restore keep the terminal panic-safe.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 7: manual verification, learning notes, README, todo

**Files:**
- Create: `docs/learning/phase-4.md`
- Modify: `README.md` (add a `live` section)
- Modify: `tasks/todo.md` (check Phase 4)

- [ ] **Step 1: Verify the real dashboard.** With a real `~/.claude/projects` present, run `cargo run -- live` in an interactive terminal: confirm the tab bar renders, `Tab` cycles Overview → Sessions → Models, the burn sparkline and cache gauge draw, and `q` restores the terminal cleanly (prompt returns, no raw-mode artifacts). Then confirm the headless path: `cargo run -- live --json | jq .command` prints `"live"`, and `cargo run -- live | head` (piped, non-TTY) emits JSON, not a TUI. If no real data exists, use `cargo run -- --dir tests/fixtures/projects live`.

- [ ] **Step 2: Write `docs/learning/phase-4.md`** covering (match the depth/style of `docs/learning/phase-3.md`): the `'static` bound on `thread::spawn` and why the worker owns its inputs (with a Go goroutine contrast); `mpsc::channel` + `recv_timeout` for a busy-sleep-free 2 s tick and clean shutdown by dropping the sender; injecting `now: DateTime<Utc>` to make burn-rate/active-session logic deterministic under test (vs calling `Utc::now()` inside); `std::io::IsTerminal` for the non-TTY snapshot branch; and `ratatui::init`/`restore` for panic-safe terminal teardown. Give file+function pointers (`src/tui/mod.rs::event_loop`, `src/tui/state.rs::burn_rate`, `src/tui/state.rs::active_project`). Include one "Rust vs Go/Swift" contrast (channels/ownership).

- [ ] **Step 3: Add a `live` section to `README.md`** after the other commands: what it shows (today's totals/cost, 10-min burn, per-model split, cache gauge, active project), the `q`/`Tab` keys, the 2 s refresh, and that `tycho live --json` (or a piped/non-TTY run) prints one snapshot for scripting/status bars.

- [ ] **Step 4: Check Phase 4 in `tasks/todo.md`.** Change `- [ ] Phase 4 — live: ratatui dashboard` to `- [x] Phase 4 — live: ratatui dashboard (2026-07-04)` and update the "Resuming from here" note to point at Phase 5.

- [ ] **Step 5: Commit.**

```bash
git add docs/learning/phase-4.md README.md tasks/todo.md
git commit -F - <<'EOF'
docs(phase-4): learning notes, README live section, gate state

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Self-Review (author checklist, run against the spec)

**Spec §5.2 coverage:**
- 2 s refresh → Task 6 worker `REFRESH` + `recv_timeout`. ✅
- Today's totals and cost → `DashboardState.today` (Task 2), Overview panel (Task 6). ✅
- Tokens/min burn over last 10 min → `burn_rate` (Task 2), sparkline (Task 6). ✅
- Per-model split → `models` (Task 2), Models tab (Task 6). ✅
- Cache hit-rate gauge → `today.hit_rate` (Task 2), `Gauge` (Task 6). ✅
- Active session via mtime + project name → Tasks 3, shown in Overview (Task 6). ✅
- `q` quit, `tab` cycle views → `handle_key` (Task 6). ✅
- `--json` on every command / non-TTY → `json::live` + `run_live` (Task 5). ✅

**Placeholder scan:** the Task 2 `session_lines`/`mark_active`/`active_project` stubs and the Task 5 `run` shim are explicitly temporary and replaced in Tasks 3/4/6; each carries the exact replacement code. No `TODO`/`TBD` remain in shipped code.

**Type consistency:** `DashboardState.today: CacheEconomics` (not a bare `Totals`) is used consistently in `json::live` (`CacheRowOut::from(&state.today)`) and the Overview panel (`state.today.actual_cost`, `state.today.hit_rate`, `state.today.totals`). `ModelLine`/`SessionLine`/`ActiveSession`/`BurnRate` field names match across state, json, and view. `Control`/`Tab`/`App` names match across app, mod, view.

**Known API risks to watch during implementation (fix inline, no plan change):**
- ratatui 0.29 `Buffer::content()` vs field `content` — plan notes the fallback.
- `Layout::vertical([...]).areas()` returns a fixed-size array via `const N`; if the array-destructuring form errors, use `.split()` and index.
- `Tabs::new` accepts an iterator of `Into<Line>`; `&str` qualifies.
- `Sparkline::data(&[u64])` — pass `&state.burn.per_minute`.
- If `Stylize::dim()`/`bold()` import path differs, use `Style::default().add_modifier(Modifier::DIM|BOLD)`.

## Execution

Plan saved to `docs/superpowers/plans/2026-07-04-live-dashboard.md`. Per the
project owner's standing correction (single approval gate on the design, which
is approved), execution proceeds inline in this session, task by task, TDD with
fmt/clippy green and a commit per task.
