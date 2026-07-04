# `tycho blocks` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `tycho blocks`, a report that groups usage into activity-anchored 5-hour billing windows with a projection for the active block.

**Architecture:** A new pure `src/blocks.rs` (mirroring `cache.rs`) folds range-filtered, cost-stamped events sorted by timestamp into blocks anchored at the first event's floored hour; a new block starts when an event lands at/after the current window's end. The block containing an injected `now` gets a linear projection. Renderers live in `report::{json,table}`; the CLI wires `Command::Blocks`.

**Tech Stack:** Rust 2024, `chrono` (`DurationRound::duration_trunc`), `rust_decimal`, existing `aggregate`/`record`/`report` modules.

## Global Constraints

- Rust stable, edition 2024, no `unsafe`, no nightly.
- No `unwrap()`/`expect()` outside `#[cfg(test)]` (clippy deny).
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean at every commit.
- Library returns typed errors; `blocks` is infallible (returns a report).
- `///` doc comments on every public item.
- Commit style: Conventional Commits with scope; author `Vinny Carpenter <vscarpenter@gmail.com>`; last line `Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr`; no Co-Authored-By footer.
- Decimal for money, floats only for token/burn estimates.

## Naming (locked; used across tasks)

- `blocks::BLOCK_HOURS: i64 = 5`
- `blocks::BlockProjection { projected_cost: Decimal, projected_tokens: u64, burn_tokens_per_min: f64, remaining: chrono::Duration }`
- `blocks::BlockTotals { start, end, first_activity, last_activity: DateTime<Utc>, totals: aggregate::Totals, models: Vec<String>, projection: Option<BlockProjection> }`
- `blocks::BlocksReport { blocks: Vec<BlockTotals>, total: Totals }`
- `blocks::blocks(events: impl IntoIterator<Item = UsageEvent>, tz: Tz, since: Option<NaiveDate>, until: Option<NaiveDate>, now: DateTime<Utc>) -> BlocksReport`
- `aggregate::in_range` becomes `pub(crate)`
- `report::json::blocks(report: &blocks::BlocksReport, timezone: &str) -> String` — top-level key `"command": "blocks"`
- `report::table::blocks(report: &blocks::BlocksReport, tz: Tz, precise: bool) -> String`
- `cli::Command::Blocks`

---

## Task 1: `blocks.rs` core (fold, split, projection)

**Files:**
- Create: `src/blocks.rs`
- Modify: `src/lib.rs` (add `pub mod blocks;`)
- Modify: `src/aggregate.rs` (make `in_range` `pub(crate)`)
- Test: `src/blocks.rs`

**Interfaces:**
- Consumes: `aggregate::{Totals, in_range}`, `record::UsageEvent`.
- Produces: everything under `blocks::` in the Naming section.

- [ ] **Step 1: Expose `in_range`.** In `src/aggregate.rs`, change `fn in_range(` to `pub(crate) fn in_range(` (keep the doc comment).

- [ ] **Step 2: Wire the module.** In `src/lib.rs`, add `pub mod blocks;` after `pub mod aggregate;`.

- [ ] **Step 3: Write `src/blocks.rs` with the tests first (types + `blocks` referenced but not yet defined).** Put this at the *top* as the module doc, then jump to Step 5 for the impl; write the test module now:

```rust
//! Billing blocks — activity-anchored 5-hour windows (Phase 5A).
//!
//! Claude's usage limits reset on rolling 5-hour windows. This report groups
//! deduplicated events into blocks anchored at the first message (floored to
//! the hour); a new block starts whenever an event lands at or after the
//! current window's end. Because the anchor is hour-floored, a >=5h inactivity
//! gap always crosses a window end, so this single check subsumes ccusage's
//! explicit gap rule. Unlike the group-by reports, block membership depends on
//! running state, so this is a fold, not a `BTreeMap` bucketing.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DedupKey, TokenUsage};

    fn ev(utc: &str, model: &str, tokens: u64, cost: &str) -> UsageEvent {
        UsageEvent {
            timestamp: utc.parse().unwrap(),
            session_id: Some("s".into()),
            project: "p".into(),
            model: model.into(),
            usage: TokenUsage {
                input: tokens,
                output: 0,
                cache_write_5m: 0,
                cache_write_1h: 0,
                cache_read: 0,
            },
            cost_usd: None,
            cost: cost.parse().unwrap(),
            dedup_key: DedupKey::Uuid(format!("{utc}-{model}")),
        }
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn events_within_five_hours_form_one_block() {
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-04T13:50:00Z", "opus", 200, "2.00"),
            ],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T20:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 1);
        let b = &report.blocks[0];
        assert_eq!(b.start, utc("2026-07-04T09:00:00Z"));
        assert_eq!(b.end, utc("2026-07-04T14:00:00Z"));
        assert_eq!(b.totals.total(), 300);
        assert_eq!(b.totals.cost, dec("3.00"));
        assert!(b.projection.is_none()); // now is past the block end
    }

    #[test]
    fn an_event_past_the_window_starts_a_new_block() {
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-04T14:30:00Z", "opus", 50, "0.50"), // >= 14:00 -> new block
            ],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T20:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 2);
        assert_eq!(report.blocks[1].start, utc("2026-07-04T14:00:00Z"));
        assert_eq!(report.total.total(), 150);
    }

    #[test]
    fn a_long_gap_starts_a_new_block() {
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-05T09:14:00Z", "opus", 100, "1.00"),
            ],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-06T00:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 2);
    }

    #[test]
    fn the_active_block_gets_a_projection() {
        // now is 2.5h into a 5h block anchored at 12:00 -> x2 scale
        let report = blocks(
            vec![ev("2026-07-04T12:00:00Z", "opus", 100, "2.00")],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T14:30:00Z"),
        );
        let b = &report.blocks[0];
        let proj = b.projection.as_ref().unwrap();
        assert_eq!(proj.projected_cost, dec("4.00"));
        assert_eq!(proj.projected_tokens, 200);
        assert_eq!(proj.remaining, Duration::minutes(150)); // 17:00 - 14:30
    }

    #[test]
    fn empty_input_yields_an_empty_report() {
        let report = blocks(vec![], chrono_tz::UTC, None, None, utc("2026-07-04T12:00:00Z"));
        assert!(report.blocks.is_empty());
        assert_eq!(report.total, Totals::default());
    }

    #[test]
    fn since_until_narrows_the_events() {
        let jul5 = NaiveDate::from_ymd_opt(2026, 7, 5).unwrap();
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-05T09:14:00Z", "opus", 200, "2.00"),
            ],
            chrono_tz::UTC,
            Some(jul5),
            None,
            utc("2026-07-06T00:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 1);
        assert_eq!(report.total.total(), 200);
    }
}
```

- [ ] **Step 4: Run, expect failure.** Run: `cargo test -p tycho-cli --lib blocks`
Expected: FAIL — types/`blocks` undefined.

- [ ] **Step 5: Implement** (insert between the module doc and `#[cfg(test)]`):

```rust
use chrono::{DateTime, Duration, DurationRound, NaiveDate, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;

use crate::aggregate::{self, Totals};
use crate::record::UsageEvent;

/// Length of a billing block, in hours.
pub const BLOCK_HOURS: i64 = 5;

/// Where the active block lands if its current rate holds to the window end.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockProjection {
    /// Cost extrapolated to the full 5-hour window.
    pub projected_cost: Decimal,
    /// Total tokens extrapolated to the full window.
    pub projected_tokens: u64,
    /// Tokens per minute over the block so far.
    pub burn_tokens_per_min: f64,
    /// Time left until the window closes.
    pub remaining: Duration,
}

/// One 5-hour billing block.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockTotals {
    /// The hour-floored anchor.
    pub start: DateTime<Utc>,
    /// `start + BLOCK_HOURS` (nominal window end).
    pub end: DateTime<Utc>,
    /// First event in the block.
    pub first_activity: DateTime<Utc>,
    /// Last event in the block.
    pub last_activity: DateTime<Utc>,
    /// Token and cost totals for the block.
    pub totals: Totals,
    /// Distinct models used, sorted.
    pub models: Vec<String>,
    /// Projection, present only on the active block.
    pub projection: Option<BlockProjection>,
}

/// The `blocks` report: activity blocks oldest-first, plus a grand total.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlocksReport {
    /// Blocks in ascending start order.
    pub blocks: Vec<BlockTotals>,
    /// Grand totals across all blocks.
    pub total: Totals,
}

/// Group events into activity-anchored 5-hour blocks. `now` marks the active
/// block (the window containing it) and drives its projection.
pub fn blocks(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> BlocksReport {
    let mut events: Vec<UsageEvent> = events
        .into_iter()
        .filter(|e| aggregate::in_range(e, tz, since, until))
        .collect();
    events.sort_by_key(|e| e.timestamp);

    let window = Duration::hours(BLOCK_HOURS);
    let mut blocks: Vec<BlockTotals> = Vec::new();
    let mut total = Totals::default();

    for event in &events {
        total.add(event);
        let start_new = blocks.last().is_none_or(|b| event.timestamp >= b.end);
        if start_new {
            let start = floor_hour(event.timestamp);
            blocks.push(BlockTotals {
                start,
                end: start + window,
                first_activity: event.timestamp,
                last_activity: event.timestamp,
                totals: Totals::default(),
                models: Vec::new(),
                projection: None,
            });
        }
        if let Some(block) = blocks.last_mut() {
            block.totals.add(event);
            block.last_activity = block.last_activity.max(event.timestamp);
            if !block.models.contains(&event.model) {
                block.models.push(event.model.clone());
            }
        }
    }

    for block in &mut blocks {
        block.models.sort();
        if block.start <= now && now < block.end {
            block.projection = Some(project(block, now));
        }
    }
    BlocksReport { blocks, total }
}

/// Truncate a timestamp down to the hour (truncation is relative to the Unix
/// epoch, which sits on an hour boundary).
fn floor_hour(ts: DateTime<Utc>) -> DateTime<Utc> {
    ts.duration_trunc(Duration::hours(1)).unwrap_or(ts)
}

/// Linear run-rate projection: scale the block's cost/tokens by
/// `window / elapsed`. Cost stays in `Decimal`; tokens and burn are floats.
fn project(block: &BlockTotals, now: DateTime<Utc>) -> BlockProjection {
    let window_secs = BLOCK_HOURS * 3_600;
    let elapsed_secs = (now - block.start).num_seconds().clamp(1, window_secs);
    let scale_dec = Decimal::from(window_secs) / Decimal::from(elapsed_secs);
    let scale_f = window_secs as f64 / elapsed_secs as f64;
    let actual_tokens = block.totals.total();
    BlockProjection {
        projected_cost: block.totals.cost * scale_dec,
        projected_tokens: (actual_tokens as f64 * scale_f).round() as u64,
        burn_tokens_per_min: actual_tokens as f64 / (elapsed_secs as f64 / 60.0),
        remaining: block.end - now,
    }
}
```

- [ ] **Step 6: Run, expect pass.** Run: `cargo test -p tycho-cli --lib blocks`
Expected: PASS (6 tests).

- [ ] **Step 7: fmt + clippy + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/lib.rs src/aggregate.rs src/blocks.rs
git commit -F - <<'EOF'
feat(blocks): activity-anchored 5-hour block folding with projection

blocks() folds range-filtered events sorted by timestamp into 5-hour windows
anchored at each first message's floored hour; the window-end check subsumes
the >=5h gap rule. The block containing an injected now gets a linear cost
projection (Decimal) plus token/burn estimates (f64).

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 2: JSON + table renderers

**Files:**
- Modify: `src/report/json.rs` (add `blocks`)
- Modify: `src/report/table.rs` (add `blocks`)
- Test: both files

**Interfaces:**
- Consumes: `blocks::{BlocksReport, BlockTotals, BlockProjection, BLOCK_HOURS}`; reuses `json::{TokensOut, rfc3339, to_json}` and `table::{money, group_thousands, new_table, human_duration}`.
- Produces: `json::blocks`, `table::blocks`.

- [ ] **Step 1: Write the failing `json::blocks` test** in `src/report/json.rs` tests module:

```rust
#[test]
fn blocks_contract() {
    use crate::blocks::{BlockProjection, BlockTotals, BlocksReport};
    let totals = Totals {
        input: 100, output: 200, cache_write_5m: 0, cache_write_1h: 0,
        cache_read: 0, cost: "3.00".parse().unwrap(),
    };
    let report = BlocksReport {
        blocks: vec![BlockTotals {
            start: "2026-07-04T12:00:00Z".parse().unwrap(),
            end: "2026-07-04T17:00:00Z".parse().unwrap(),
            first_activity: "2026-07-04T12:05:00Z".parse().unwrap(),
            last_activity: "2026-07-04T14:20:00Z".parse().unwrap(),
            totals,
            models: vec!["claude-opus-4-8".into()],
            projection: Some(BlockProjection {
                projected_cost: "6.00".parse().unwrap(),
                projected_tokens: 600,
                burn_tokens_per_min: 2.0,
                remaining: chrono::Duration::minutes(160),
            }),
        }],
        total: totals,
    };
    let value: serde_json::Value =
        serde_json::from_str(&blocks(&report, "UTC")).unwrap();
    assert_eq!(value["command"], "blocks");
    let b = &value["blocks"][0];
    assert_eq!(b["start"], "2026-07-04T12:00:00Z");
    assert_eq!(b["end"], "2026-07-04T17:00:00Z");
    assert_eq!(b["tokens"]["total"], 300);
    assert_eq!(b["models"][0], "claude-opus-4-8");
    assert_eq!(b["active"], true);
    assert_eq!(b["projection"]["projected_cost_usd"], 6.0);
    assert_eq!(b["projection"]["remaining_seconds"], 9_600);
    assert_eq!(value["totals"]["total"], 300);
}
```

- [ ] **Step 2: Run, expect failure.** Run: `cargo test -p tycho-cli --lib blocks_contract`
Expected: FAIL — `blocks` undefined in `json`.

- [ ] **Step 3: Implement `json::blocks`.** Append to `src/report/json.rs` before the `#[cfg(test)]` module:

```rust
#[derive(Serialize)]
struct ProjectionOut {
    projected_cost_usd: f64,
    projected_tokens: u64,
    burn_tokens_per_min: f64,
    remaining_seconds: i64,
}

#[derive(Serialize)]
struct BlockOut {
    start: String,
    end: String,
    first_activity: String,
    last_activity: String,
    tokens: TokensOut,
    models: Vec<String>,
    active: bool,
    projection: Option<ProjectionOut>,
}

#[derive(Serialize)]
struct BlocksOut {
    command: &'static str,
    timezone: String,
    blocks: Vec<BlockOut>,
    totals: TokensOut,
}

/// Render the billing-blocks report as pretty-printed JSON.
pub fn blocks(report: &crate::blocks::BlocksReport, timezone: &str) -> String {
    use rust_decimal::prelude::ToPrimitive;
    let out = BlocksOut {
        command: "blocks",
        timezone: timezone.to_owned(),
        blocks: report
            .blocks
            .iter()
            .map(|b| BlockOut {
                start: rfc3339(b.start),
                end: rfc3339(b.end),
                first_activity: rfc3339(b.first_activity),
                last_activity: rfc3339(b.last_activity),
                tokens: TokensOut::from(&b.totals),
                models: b.models.clone(),
                active: b.projection.is_some(),
                projection: b.projection.as_ref().map(|p| ProjectionOut {
                    projected_cost_usd: p.projected_cost.to_f64().unwrap_or(0.0),
                    projected_tokens: p.projected_tokens,
                    burn_tokens_per_min: p.burn_tokens_per_min,
                    remaining_seconds: p.remaining.num_seconds(),
                }),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    to_json(&out)
}
```

- [ ] **Step 4: Run, expect pass.** Run: `cargo test -p tycho-cli --lib blocks_contract`
Expected: PASS.

- [ ] **Step 5: Write the failing `table::blocks` test** in `src/report/table.rs` tests module:

```rust
#[test]
fn blocks_renders_window_status_and_active_projection() {
    use crate::blocks::{BlockProjection, BlockTotals, BlocksReport};
    let totals = Totals {
        input: 100, output: 200, cache_write_5m: 0, cache_write_1h: 0,
        cache_read: 0, cost: "3.00".parse().unwrap(),
    };
    let report = BlocksReport {
        blocks: vec![BlockTotals {
            start: "2026-07-04T12:00:00Z".parse().unwrap(),
            end: "2026-07-04T17:00:00Z".parse().unwrap(),
            first_activity: "2026-07-04T12:05:00Z".parse().unwrap(),
            last_activity: "2026-07-04T14:20:00Z".parse().unwrap(),
            totals,
            models: vec!["opus".into()],
            projection: Some(BlockProjection {
                projected_cost: "6.00".parse().unwrap(),
                projected_tokens: 600,
                burn_tokens_per_min: 2.0,
                remaining: chrono::Duration::minutes(160),
            }),
        }],
        total: totals,
    };
    let rendered = blocks(&report, chrono_tz::UTC, false);
    assert!(rendered.contains("Active block:"), "{rendered}");
    assert!(rendered.contains("07-04 12:00"), "{rendered}");
    assert!(rendered.contains("active"), "{rendered}");
    assert!(rendered.contains("$6.00"), "{rendered}");
    assert!(rendered.contains("Total"), "{rendered}");
}
```

- [ ] **Step 6: Run, expect failure.** Run: `cargo test -p tycho-cli --lib blocks_renders`
Expected: FAIL — `blocks` undefined in `table`.

- [ ] **Step 7: Implement `table::blocks`.** Append to `src/report/table.rs` before the `#[cfg(test)]` module:

```rust
/// Render the billing-blocks report: an optional active-block headline, one
/// row per block, and a totals row.
pub fn blocks(report: &crate::blocks::BlocksReport, tz: Tz, precise: bool) -> String {
    let active = report
        .blocks
        .iter()
        .find_map(|b| b.projection.as_ref().map(|p| (b, p)));
    let headline = match active {
        Some((b, p)) => format!(
            "Active block: {} so far, ~{} projected by {} ({:.0} tok/min).\n\n",
            money(b.totals.cost, precise),
            money(p.projected_cost, precise),
            b.end.with_timezone(&tz).format("%H:%M"),
            p.burn_tokens_per_min,
        ),
        None => String::new(),
    };

    let mut table = new_table(["Block (5h)", "Status", "Models", "Total Tokens", "Cost (USD)"]);
    for b in &report.blocks {
        let window = format!(
            "{} – {}",
            b.start.with_timezone(&tz).format("%m-%d %H:%M"),
            b.end.with_timezone(&tz).format("%H:%M"),
        );
        let (status, cost) = match &b.projection {
            Some(p) => (
                format!("● active · {} left", human_duration(p.remaining)),
                format!(
                    "{} → ~{}",
                    money(b.totals.cost, precise),
                    money(p.projected_cost, precise)
                ),
            ),
            None => (format!("{BLOCK_HOURS}h"), money(b.totals.cost, precise)),
        };
        table.add_row(vec![
            window,
            status,
            b.models.join(", "),
            group_thousands(b.totals.total()),
            cost,
        ]);
    }
    table.add_row(vec![
        "Total".to_owned(),
        String::new(),
        String::new(),
        group_thousands(report.total.total()),
        money(report.total.cost, precise),
    ]);
    format!("{headline}{table}")
}
```

Add the import at the top of `src/report/table.rs`: `use crate::blocks::BLOCK_HOURS;`.

- [ ] **Step 8: Run, expect pass.** Run: `cargo test -p tycho-cli --lib blocks_renders`
Expected: PASS.

- [ ] **Step 9: fmt + clippy + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/report/json.rs src/report/table.rs
git commit -F - <<'EOF'
feat(blocks): JSON and table renderers

json::blocks emits the documented contract (blocks[] with tokens/models/
active/projection + totals); table::blocks prints the active-block headline,
one row per window with status + cost (projected on the active row), and a
totals row.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 3: CLI command + `main.rs` wiring + integration test

**Files:**
- Modify: `src/cli.rs` (add `Command::Blocks`)
- Modify: `src/main.rs` (render arm)
- Test: `tests/cli.rs`

**Interfaces:**
- Consumes: `blocks::blocks`, `json::blocks`, `table::blocks`, `reject_csv`.
- Produces: `tycho blocks` end to end.

- [ ] **Step 1: Add the command** in `src/cli.rs`, in the `Command` enum after `Live`:

```rust
    /// Per 5-hour billing block: tokens, cost, and the active block's projection
    Blocks,
```

- [ ] **Step 2: Wire `render()`** in `src/main.rs`. Add an arm before `Command::Live` (which stays `unreachable!`):

```rust
        Command::Blocks => {
            reject_csv(global.csv);
            let report = tycho::blocks::blocks(outcome.events, tz, since, until, Utc::now());
            if global.json {
                json::blocks(&report, tz.name())
            } else {
                table::blocks(&report, tz, global.precise)
            }
        }
```

`Utc` is already imported in `main.rs` (from the `live` work).

- [ ] **Step 3: Write the integration test** in `tests/cli.rs`:

```rust
#[test]
fn blocks_json_groups_usage_into_windows() {
    let value = stdout_json(tycho().args(["blocks", "--json"]));
    assert_eq!(value["command"], "blocks");
    let blocks = value["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty());
    let b0 = &blocks[0];
    assert!(b0["start"].is_string());
    assert!(b0["end"].is_string());
    assert!(b0["tokens"]["total"].is_number());
    // Fixtures are all in the past, so no active block.
    assert_eq!(b0["active"], false);
    // Grand total matches the whole fixture corpus (see daily test).
    assert_eq!(value["totals"]["total"], 1_510);
}

#[test]
fn blocks_csv_is_a_usage_error() {
    tycho().args(["blocks", "--csv"]).assert().code(2);
}
```

- [ ] **Step 4: Run, expect pass.** Run: `cargo test -p tycho-cli blocks`
Expected: PASS (unit + integration).

- [ ] **Step 5: fmt + clippy + full test + commit.**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test -p tycho-cli
git add src/cli.rs src/main.rs tests/cli.rs
git commit -F - <<'EOF'
feat(blocks): wire the blocks command end to end

Adds Command::Blocks and the main.rs render arm (table/--json, --csv
rejected). Integration test confirms the JSON contract against the fixtures.

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Task 4: docs — README, learning notes, todo

**Files:**
- Modify: `README.md`
- Create: `docs/learning/phase-5.md`
- Modify: `tasks/todo.md`

- [ ] **Step 1: README.** Add a row to the command table after the `live` row:

```markdown
| `tycho blocks` | Per 5-hour billing block: tokens, cost, models, and the active block's projected total |
```

And a short paragraph after the `tycho live` section explaining activity-anchored 5-hour windows and the active-block projection.

- [ ] **Step 2: Verify the real output** before writing the learning note. Run: `cargo run -- --dir tests/fixtures/projects --utc blocks` and `cargo run -- blocks` (real data, if present). Confirm the table renders windows and a totals row.

- [ ] **Step 3: `docs/learning/phase-5.md`.** Cover (match the depth of `phase-4.md`): the fold-vs-group-by distinction (`src/blocks.rs::blocks` carries running state where `aggregate::daily` uses a pure `BTreeMap` key); `chrono::DurationRound::duration_trunc` for hour-flooring; the single-window-condition simplification (why hour-flooring subsumes the gap rule); Decimal-for-cost / f64-for-tokens split in `project`; and injected `now`. Include file+function pointers. One Rust-vs-Go/Swift contrast.

- [ ] **Step 4: `tasks/todo.md`.** Under the Phase 5 area, record 5A done: `tycho blocks` shipped, test count, and that 5B (cargo-dist) is next.

- [ ] **Step 5: Commit.**

```bash
git add README.md docs/learning/phase-5.md tasks/todo.md
git commit -F - <<'EOF'
docs(phase-5a): blocks in README, phase-5 learning notes, gate state

Claude-Session: https://claude.ai/code/session_01BSnnx4maE2qmptBM1AJXQr
EOF
```

---

## Self-Review

**Spec coverage:**
- Activity-anchored 5h blocks + window-end split → Task 1 `blocks()`. ✅
- Active block + projection (injected `now`) → Task 1 `project()`. ✅
- `Command::Blocks`, global filters, `--json`, no `--csv` → Task 3. ✅
- Table (headline, window/status/models/tokens/cost, totals) + JSON contract → Task 2. ✅
- Unit + integration tests → Tasks 1–3. ✅
- README + learning note + todo → Task 4. ✅

**Placeholder scan:** none — every code step is complete.

**Type consistency:** `BlockTotals`/`BlockProjection`/`BlocksReport` field names and `blocks()`/`project()` signatures match across Tasks 1–3; `json`/`table` read `b.projection`, `b.totals`, `p.projected_cost`, `p.remaining` exactly as defined. `active` in JSON derives from `projection.is_some()` consistently with `table`.

**API risks to watch (fix inline, no plan change):** `DurationRound` must be imported for `duration_trunc`; `Decimal::from_f64_retain` is not used (kept in Decimal via integer ratio); `is_none_or` is already used elsewhere in the crate, so it is available.

## Execution

Inline, task by task, TDD with fmt/clippy green and a commit per task (per the owner's single-design-gate workflow).

