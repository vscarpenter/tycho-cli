# Phase 4 — Rust concepts this phase exercised

## 1. `thread::spawn` needs `'static`, so the worker *owns* its inputs

**Where:** `src/tui/mod.rs::event_loop` — the `thread::spawn(move || { … })` worker.

The dashboard refreshes on a background thread while the UI thread draws. The
first instinct is to let the worker read `roots`/`pricing` from `run`'s stack,
but `thread::spawn` requires an `F: 'static` closure: the thread may outlive the
function that spawned it, so it cannot borrow anything from that stack frame.
The compiler's fix is to `move` the data into the closure — the worker *owns*
`roots`, `project`, `pricing`. That single bound is what shapes the whole
architecture: because the worker can't share mutable state with the UI thread,
the finished `DashboardState` flows *back* over a channel instead. Go's
goroutines capture by reference and will happily let two goroutines touch the
same slice (you find the race at runtime, with `-race`, if you're lucky); Rust
turns "who owns this across the thread boundary?" into a compile error you must
answer before it builds.

## 2. `mpsc` + `recv_timeout`: a timer and a shutdown signal in one

**Where:** `src/tui/mod.rs::event_loop` — the `snap_tx`/`snap_rx` and
`stop_tx`/`stop_rx` channels.

Two channels carry the whole concurrency design. The worker sends snapshots
down `snap_tx`; the UI drains them with non-blocking `snap_rx.try_recv()` each
frame. The clever half is the *second* channel: the worker's 2-second wait is
`stop_rx.recv_timeout(REFRESH)`, not `thread::sleep`. On `Err(Timeout)` it loops
and re-scans; on anything else it exits. That "anything else" is the shutdown
path — when the UI thread `drop`s `stop_tx`, the channel disconnects and
`recv_timeout` returns `Err(Disconnected)` immediately, so the worker stops
within one wait instead of sleeping out a full interval. No busy-loop, no
`AtomicBool` polling, no shared flag: dropping a sender *is* the signal. The
symmetric case matters too — if the UI dies first, the worker's `snap_tx.send`
returns `Err` and it breaks. Neither thread can wedge the other.

## 3. Injecting `now` makes time-dependent logic testable

**Where:** `src/tui/state.rs::burn_rate`, `src/tui/state.rs::active_project`,
`DashboardState::derive` (all take `now: DateTime<Utc>`).

"Tokens per minute over the last 10 minutes" and "active if touched in the last
5 minutes" are clock-dependent — the temptation is to call `Utc::now()` inside
them. That makes them untestable: the answer changes every run. Instead `now` is
a *parameter*, supplied by the event loop as `Utc::now()` in production and as a
fixed `"2026-07-04T12:00:00Z"` in tests. The burn-rate test can then assert an
exact sparkline bucket, and the active-session test can place a file mtime
"30 seconds ago" and know the answer. This is dependency injection without a
framework — just passing the value instead of fetching it — and it is why the
`tui::state` tests have zero flakiness despite being entirely about elapsed
time.

## 4. `std::io::IsTerminal` — one trait, no dependency

**Where:** `src/main.rs::run_live` — `std::io::stdout().is_terminal()`.

`live` must not spew raw-mode escape codes into a pipe. The check is a single
stable-std trait method: `stdout().is_terminal()` is `false` when stdout is
redirected or piped, so those runs (and `--json`) take the one-shot JSON
snapshot branch and only a real terminal starts the TUI. No `atty` crate, no
`libc` — this moved into `std` in 1.70. Go reaches for
`golang.org/x/term.IsTerminal(int(os.Stdout.Fd()))`; Rust now has it in the
standard prelude of traits.

## 5. Panic-safe terminal teardown with `ratatui::init` / `restore`

**Where:** `src/tui/mod.rs::run`.

A TUI puts the terminal into raw mode and an alternate screen; if the program
panics without undoing that, it leaves the user's shell wrecked (no echo, no
cursor). `ratatui::init()` enters raw mode + alternate screen *and installs a
panic hook that restores them first*, so even a panic mid-render leaves a clean
terminal with a readable backtrace. `run` then pairs it with an explicit
`ratatui::restore()` after the loop, on both the `Ok` and `Err` paths, so a
returned `io::Error` also restores. This is the RAII idea — cleanup tied to a
known point — reinforced by the hook for the abnormal path. Swift/Go would reach
for `defer`; here the library bundles the correct hook so you don't hand-roll
it, and the PTY check at the gate confirmed it: the app entered the alternate
screen (`1049h`) and left it (`1049l`) on `q`, exit code 0.

## Borrow-checker moment

The honest fight this phase was ordering, not lifetimes. In `compute_snapshot`
the discovered `files` are needed twice: `collect_mtimes(&files)` borrows them,
then `scan_files(files, …)` *consumes* them. Written in the other order, the
borrow for mtimes would still be live when `scan_files` tried to move `files`,
and the compiler rejects it. The fix is just sequencing — take the shared borrow
and finish with it, *then* hand ownership to the consumer — but it is a small
daily reminder that "read it, then give it away" is an order the compiler
actually checks, where Go/Swift would let both happen and trust you.
