# Phase 5A design — `tycho blocks` (5-hour billing-block report)

Status: approved 2026-07-04. First sub-project of Phase 5 (spec §10 "Ship",
the "5-hour billing-blocks report"). The single approval gate was the design
below; spec → plan → implementation then runs in one continuous pass.

## Goal

A `tycho blocks` command that groups usage into activity-anchored 5-hour
billing blocks (mirroring Claude's rolling reset windows, ccusage-style):
per-block tokens and cost, the models used, and — for the block containing
"now" — a linear projection of where the block's cost lands by its end.

## Command & flags

- New `Command::Blocks`, wired like the other reports.
- Honors every global flag: `--since`/`--until` (narrow by local date),
  `--project`, `--model`, `--dir`, `--mode`, `--pricing`, `--tz`/`--utc`,
  `--precise`.
- `--json` is a documented contract (`command: "blocks"`).
- **No `--csv`** (matches the rollup reports `projects`/`models`/`cache`;
  `reject_csv` guards it). Trivial to add later.
- Shows every block that has activity, oldest → newest, the active block
  marked and projected.

## Block-building algorithm (activity-anchored)

Same front of the pipeline as every report: discover → parse → dedupe → cost
stamp → range-filter. Then, over the range-filtered events sorted by
timestamp, a **fold with carried state**:

1. **Anchor.** The first event's timestamp, floored to the hour (UTC), is
   `block_start`. The nominal window is `[block_start, block_start + 5h)`.
2. **New block** when an event's timestamp is `>= block_start + 5h`. It
   re-anchors: the new `block_start` is that event's floored hour.
3. Otherwise the event joins the current block (fold its usage/cost into the
   block's `Totals`, extend `last_activity`, record its model).

### Why one condition, not two

ccusage's rule is "new block when 5h elapsed since block start **or** ≥5h
since the last entry." With hour-floored anchoring the two coincide: any event
`≥5h` after the previous event is necessarily `≥5h` after `block_start` (the
previous event is `≥ block_start`), so it already trips the window condition.
The implementation therefore uses the single window check — equivalent
behavior, less code. No explicit "gap blocks" are emitted; a long idle gap
simply ends one block and the next event anchors the following one.

### Why a fold, not a group-by

`daily`/`monthly`/`models` bucket by a pure key (date, month, model) into a
`BTreeMap`. Blocks cannot: whether an event opens a new block depends on the
**running state** (the current `block_start`), so it is a stateful fold/scan,
not a group-by. This contrast is the Phase 5 learning note.

## Active block & projection

The block whose window `[block_start, block_start + 5h)` contains the injected
`now` is **active** — the same `now`-injection discipline as `live`, so the
projection is deterministic under test.

For the active block:
- `elapsed = now − block_start`, clamped to `(0, 5h]`.
- `projected_cost = actual_cost × (5h / elapsed)` (linear run-rate).
- `projected_tokens = round(total_tokens × 5h / elapsed)`.
- `burn_tokens_per_min = total_tokens / elapsed_minutes`.
- `remaining = block_end − now`.

If `now` is at or past the last block's end (no window contains it), there is
no active block and no projection.

## Data model & module

New `src/blocks.rs` (mirrors `src/cache.rs`), pure and TDD:

```rust
pub struct BlockProjection {
    pub projected_cost: Decimal,
    pub projected_tokens: u64,
    pub burn_tokens_per_min: f64,
    pub remaining: chrono::Duration,
}

pub struct BlockTotals {
    pub start: DateTime<Utc>,          // floored anchor
    pub end: DateTime<Utc>,            // start + 5h (nominal window end)
    pub first_activity: DateTime<Utc>, // first event in the block
    pub last_activity: DateTime<Utc>,  // last event in the block
    pub totals: Totals,                // reused from aggregate
    pub models: Vec<String>,           // distinct, sorted
    pub projection: Option<BlockProjection>, // Some only on the active block
}

pub struct BlocksReport {
    pub blocks: Vec<BlockTotals>,
    pub total: Totals,
}

pub const BLOCK_HOURS: i64 = 5;

pub fn blocks(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> BlocksReport;
```

Reuses `aggregate::Totals`. The range filter (`aggregate::in_range`) is made
`pub(crate)` so `blocks.rs` filters events by local-date range exactly as the
other reports do — filter first, then fold (a partial block at the `--since`
boundary is acceptable and consistent with how `daily` shows a partial day).

## Rendering

`report::table::blocks` and `report::json::blocks`:

- **Table** columns: Block window (start–end in `tz`, `%m-%d %H:%M`) · Status
  (`5h` for a closed block, `● active · 2h10m left` for the active one) ·
  Models · Total Tokens · Cost. The active row appends its projection to the
  cost cell (`$2.05 → ~$4.10`). A Totals row closes the table.
- **Headline** printed above the table when a block is active:
  `Active block: $X so far, ~$Y projected by HH:MM (N tok/min).`
- **JSON**: `blocks[]` each with `start`, `end`, `first_activity`,
  `last_activity`, `tokens` (the shared `TokensOut` block), `models`,
  `active` (bool), and `projection` (object or null); plus `totals` and
  `command`/`timezone`.

## Testing

- **Unit (`blocks.rs`, fixed `now`)**: two events 5h apart split into two
  blocks; events inside one window stay together; a multi-hour gap ends a
  block; active-block detection when `now` is inside the last window;
  projection math (`elapsed = 2.5h` doubles the cost); empty input yields an
  empty report; `--since`/`--until` narrowing.
- **Integration (`tests/cli.rs`)**: `tycho blocks --json` against the fixtures
  produces valid JSON with the documented keys and correct block count.
- fmt/clippy(`-D warnings`) clean at every commit; TDD throughout.

## Docs

- README: a `blocks` row in the command table and a short paragraph.
- `docs/learning/phase-5.md`: the fold-vs-group-by distinction, the
  single-condition simplification, `chrono::DurationRound::duration_trunc`
  for hour-flooring, and the injected-`now` projection.
- `tasks/todo.md`: record the Phase 5A gate.

## Scope guard (YAGNI)

No `--csv`, no `--active`-only flag, no token-quota percentage, no explicit
idle "gap" rows. Just the blocks with activity and the active projection.
