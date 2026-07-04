# Phase 5 — Rust concepts this phase exercised

## 5A · Billing blocks

## 1. A fold with carried state, not a group-by

**Where:** `src/blocks.rs::blocks` vs `src/aggregate.rs::daily`.

Every other report buckets by a **pure key**: `daily` maps each event to its
local date and drops it into a `BTreeMap<NaiveDate, Totals>` — the bucket an
event lands in depends only on that event. Blocks can't work that way. Whether
an event opens a new block depends on the *block that's currently open* — the
running `block_start`. So `blocks` is a **fold**: iterate events in timestamp
order, carry the in-progress block in `blocks.last()`, and decide per event
whether to push a new one. The tell is that the decision reads prior state
(`blocks.last().is_none_or(|b| event.timestamp >= b.end)`) — a `BTreeMap` key
never can. Recognizing "is this a group-by or a scan?" is the core modeling
call, and Rust makes the difference concrete: the group-by is a `collect` into
a map; the scan is an explicit `for` loop mutating an accumulator.

## 2. Borrowing `last()` then `last_mut()` in one pass

**Where:** `src/blocks.rs::blocks`, the per-event loop.

The loop reads `blocks.last()` (shared borrow) to decide `start_new`, may
`push` (which needs the vec free of borrows), then takes `blocks.last_mut()`
(exclusive borrow) to fold the event in. Rust allows this only because the
three borrows are **sequential, not overlapping** — each ends before the next
begins. Writing it as one `if let Some(block) = blocks.last_mut()` guard also
sidesteps the banned `unwrap()`: after a conditional `push`, "the vec is
non-empty" is something *I* know but the compiler doesn't, so instead of
asserting it with `unwrap`, the `if let` simply handles both arms. A Go slice
would let me index `blocks[len-1]` and never think about it; here the borrow
checker makes the read-then-mutate ordering explicit, which is exactly the bug
class (mutating while iterating) it exists to prevent.

## 3. Proving two rules are one: hour-flooring subsumes the gap check

**Where:** `src/blocks.rs::blocks` (the single `event.timestamp >= b.end`
check) and `floor_hour`.

ccusage starts a new block when "5h elapsed since block start **or** ≥5h since
the last entry." I implemented only the first and dropped the second — because
with hour-floored anchoring they're provably identical. The last entry is
always `>= block_start`, so any event `≥5h` after it is `≥5h` after
`block_start`, i.e. already at/after `block_start + 5h = b.end`. The window
check fires first every time. This is the kind of simplification worth writing
down: not "I skipped a rule," but "I proved the rule was redundant given the
anchor." Fewer branches, same behavior — and the unit tests
(`a_long_gap_starts_a_new_block`) pin it.

## 4. `duration_trunc` for hour-flooring

**Where:** `src/blocks.rs::floor_hour`.

`ts.duration_trunc(Duration::hours(1))` snaps a timestamp down to the hour.
The subtlety worth knowing: truncation is **relative to the Unix epoch**, and
the epoch sits exactly on an hour (and day) boundary, so truncating to one hour
lands on `:00:00` as intended. It returns `Result` (a duration that didn't
divide evenly would error), so `.unwrap_or(ts)` keeps the no-`unwrap` rule —
an hour is always a clean divisor here, but degrading to the original instant
is a safe, panic-free fallback. Comes from the `DurationRound` trait, which
must be imported for the method to resolve — a recurring Rust surprise (the
method exists only when its trait is in scope).

## 5. Decimal for the ledger, f64 for the estimate — in one function

**Where:** `src/blocks.rs::project`.

The projection scales the block by `window / elapsed`. Cost is money, so it
scales in `Decimal` (`block.totals.cost * Decimal::from(window_secs) /
Decimal::from(elapsed_secs)` — an exact rational). Projected *tokens* and the
burn rate are estimates of a fundamentally noisy quantity (output tokens are
already undercounted), so they scale in `f64` and round. The same function
deliberately uses two number types for two audiences — the exact one where a
reader would notice a rounding drift, the fast one where they wouldn't. That's
the Phase 3 "Decimal for ledgers, floats for graphs" doctrine applied at the
granularity of a single computation.

## Note on `now`

Like the `live` dashboard, `blocks` takes `now: DateTime<Utc>` as a parameter
rather than calling `Utc::now()`, so `the_active_block_gets_a_projection` can
place `now` exactly 2.5 hours into a window and assert the cost doubles. The
binary passes `Utc::now()`; the tests pass a literal. Same discipline, second
outing — see `docs/learning/phase-4.md §3`.
