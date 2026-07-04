# Phase 4 design — `tycho live` (ratatui dashboard)

Status: approved 2026-07-04. Implements spec §5.2. The single approval gate
(per the project owner's working agreement) was the design below; spec → plan
→ implementation then runs in one continuous pass.

## Goal

A ratatui dashboard, `tycho live`, that refreshes every 2 seconds and shows:
today's totals and cost, a tokens-per-minute burn rate over the last 10
minutes, a per-model split, a cache hit-rate gauge, and the active project
(detected via recent file mtime). Keys: `q` quit, `Tab` cycle views. It is a
monitor, not an app.

## Module layout (spec §6 names it `tui`)

```
src/tui/
  mod.rs     run(), the event loop, terminal setup/teardown, panic-safe
             restore, and the pure key handler
  state.rs   DashboardState — the pure, testable core; derivation from
             (events, file mtimes, now, tz, pricing)
  view.rs    ratatui rendering of DashboardState into a frame, one function
             per tab
```

`src/lib.rs` gains `pub mod tui;`.

Dependency: add **`ratatui`** and use its re-exported **`ratatui::crossterm`**
(one version, no skew). TTY detection uses `std::io::IsTerminal` (stable) — no
extra crate.

## The testable core: `DashboardState`

A small aggregated snapshot produced by a **pure** function:

```rust
DashboardState::derive(events, mtimes, now, tz, pricing) -> DashboardState
```

Everything time-relative takes an **injected `now: DateTime<Utc>`** — the logic
never calls `Utc::now()` itself, so tests are deterministic (the same
discipline the rest of the codebase already follows).

Fields:

- `today: Totals` — today's totals + stamped cost (local calendar day in `tz`).
- `today_cache: CacheEconomics` — reuse `cache::economics` on today's events;
  drives the hit-rate gauge and the leverage/savings line.
- `burn: BurnRate { tokens_per_min: f64, cost_per_hour: Decimal, per_minute:
  [u64; 10] }` — last-10-minute window; `per_minute` (oldest → newest) feeds a
  ratatui `Sparkline`.
- `models: Vec<ModelLine>` — today's per-model tokens/cost/hit-rate (reuse
  `aggregate::models`, hit rate from cache economics).
- `sessions: Vec<SessionLine>` — today's sessions (reuse `aggregate::sessions`),
  newest first, one flagged `active`.
- `active: Option<ActiveSession { project, session_id, idle }>` — from mtime.
- `generated_at: DateTime<Utc>` — drives an "updated Ns ago" footer.

The three tabs are just views over this one struct; no view fetches its own
data.

## Active-session detection (spec: "via recent file mtime")

I/O is split from logic so the logic is testable without touching the clock or
the filesystem — mirroring how `discover` splits pure `default_roots` from I/O
`discover`:

- `active_project(mtimes: &[(String /* project */, SystemTime)], now,
  threshold) -> Option<ActiveSession>` — **pure**: the newest mtime within
  `threshold` wins; its project is "active." Unit-tested with hand-built
  tuples.
- `collect_mtimes(&[TranscriptFile]) -> Vec<(String, SystemTime)>` — the thin
  I/O helper doing `fs::metadata().modified()`.

`threshold` defaults to **5 minutes**, a named constant (`ACTIVE_THRESHOLD`).
The Sessions view marks the newest session as `active` when an active project
exists. mtime is a more sensitive "something is happening now" signal than the
last *assistant* event (non-assistant records touch the file too), which is why
the spec chose it.

## One clean seam in `scan` (motivated, small)

`live` needs the discovered file list to read mtimes. To avoid walking the tree
twice per tick, extract `scan::scan_files(files, filter)` and redefine today's
`scan(roots, filter)` as `scan_files(discover(roots)…filtered by project,
filter)`. The worker then does **one** walk per tick:

```
discover(roots) → filter by project → collect_mtimes + scan_files
```

This is the only change to existing code; it is a reusable seam, not gratuitous
refactoring. `active_project` and `scan_files` both consume the
project-filtered file list (so `--project` narrows "active" too): compute
`active` (borrows `&files`) before moving `files` into `scan_files`.

## Refresh architecture — background scan thread + channel

```
UI thread (mod.rs):                    Worker thread:
  setup terminal + panic hook            loop {
  spawn worker, hold stop_tx               scan → Coster::apply (stamp costs)
  loop {                                   state = derive(events, mtimes, Utc::now(), …)
    draw(latest, tab)                      if tx.send(state).is_err() { break }
    if poll(16ms): handle_key              match stop_rx.recv_timeout(2s) {
    drain rx.try_recv() -> latest            Err(Timeout) => continue,   // work
  }                                          _            => break,      // shutdown
  drop stop_tx (worker exits), join        }
                                         }
```

- The worker computes **before** waiting, so the first snapshot arrives in
  milliseconds; the UI shows "Scanning…" until then.
- `stop_rx.recv_timeout(2s)` is a clean, std-only shutdown with no busy-sleep:
  dropping `stop_tx` on the UI thread disconnects the channel, and the worker's
  next `recv_timeout` returns `Disconnected` and exits.
- Keys: `q` / `Ctrl-C` quit; `Tab` cycles the three tabs.
- Key handling is a pure `handle_key(key, &mut App) -> Control { Continue |
  Quit }`, unit-testable without a terminal.

### The `'static` teaching moment (docs/learning/phase-4.md)

`thread::spawn` requires a `'static` closure, so the worker cannot borrow
`roots`/`pricing` from `run`'s stack — it must **own** them (clone in). That
compiler requirement is exactly why the snapshot flows back over a channel
instead of the UI thread reaching into shared mutable state. Rust-vs-Go
contrast: goroutines capture by reference and let you race; Rust's `'static`
bound forces the ownership transfer to be explicit.

## Non-TTY and `--json` (spec §5: "--json on every command", "detect non-TTY")

`tycho live --json`, **or** any run where stdout is not a TTY, computes **one**
`DashboardState` snapshot, prints it via a new `json::live(...)`, and exits 0.
Only an interactive TTY without `--json` starts the dashboard. This satisfies
both spec requirements and makes the whole state end-to-end testable headlessly.

- `--csv` is rejected on `live` (like other non-tabular commands).
- `--since` / `--until` are ignored by `live` (it is inherently "now");
  documented, not an error.
- Honors `--dir` / `--project` / `--model` / `--mode` / `--pricing` / `--tz` /
  `--utc`.
- The interactive TUI prints **nothing** to stderr (the per-model
  unknown-model warnings `main.rs` emits would corrupt the alternate screen):
  unpriced models are priced at zero as everywhere else and remain
  discoverable via `doctor`. The `--json` / non-TTY snapshot path may warn to
  stderr as usual.

## Testing (TDD throughout)

- **state.rs**: fixed `now` + synthetic events/mtimes → assert today's totals,
  burn rate + sparkline buckets, model/session lines, active detection in/out
  of threshold, cache hit rate.
- **active_project**: pure, hand-built `(project, SystemTime)` tuples — no
  `filetime` dev-dependency needed.
- **handle_key**: `q` → Quit; `Tab` → tab index advances and wraps.
- **view.rs**: render each tab into a `ratatui::backend::TestBackend` buffer
  and assert key strings (model name, `$`, `active`) appear — a cheap
  regression guard, no `insta`.
- **tests/cli.rs**: `tycho live --json` against fixtures → valid JSON with the
  documented keys; piped (non-TTY) `live` → same. Whole path, headless.

## Terminal safety

A panic hook restores the terminal (disable raw mode, leave the alternate
screen) *before* the default hook prints, paired with an RAII guard for the
normal/unwind path — the classic ratatui gotcha, and a learning note.

## Deliverables

1. `ratatui` dependency; `src/tui/{mod,state,view}.rs`; `scan::scan_files`
   seam; `json::live`; `live` wired into `cli.rs` / `main.rs`.
2. Unit + integration tests as above; `cargo fmt --check` and `cargo clippy
   --all-targets -- -D warnings` clean at every commit.
3. `docs/learning/phase-4.md` (threads / mpsc / `recv_timeout`, the `'static`
   bound, panic-safe TUI, injecting `now`, Rust-vs-Go).
4. README `live` section; `tasks/todo.md` Phase 4 checkbox.
5. Manual verification of the real dashboard (q / Tab / 2s refresh) plus the
   automated `--json` path.

## Scope guard (YAGNI)

No config for thresholds/refresh interval (named consts), no mouse, no
scrolling, no historical charts beyond the 10-minute sparkline. "A monitor, not
an app."
