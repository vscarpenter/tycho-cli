# Phase 2 — Rust concepts this phase exercised

## 1. Data parallelism with rayon (vs Go's goroutines)

**Where:** `src/scan.rs::scan` — `files.into_par_iter().map(parse).collect()`.

One changed line made the scan parallel: `into_iter` → `into_par_iter`.
No goroutines, no channels, no WaitGroup, no mutex. Rayon's work-stealing
pool splits the file list across cores, and *the compiler* proves safety:
a closure sent to another thread must only capture `Send` data, so the
data race Go would let you write (and catch, maybe, with `-race` at
runtime) is a compile error here. The design cost: the merge loop stays
sequential so the `Deduper` needs no lock — parallelize the embarrassingly
parallel part (parsing), keep the stateful part (dedup) single-threaded.
Result: 549 MB went from 1.63 s to 0.09 s.

## 2. Layer separation via `From` conversions

**Where:** `src/cli.rs` — `impl From<SortKey> for aggregate::SessionSort`.

`SortKey` derives `clap::ValueEnum`; `SessionSort` lives in the aggregate
module and knows nothing about clap. They have identical variants — on
purpose. The `From` impl is the one place the CLI layer touches the core
layer, and `sort.into()` at the call site keeps the dependency pointing
the right way: the library never imports clap types. In Go you'd likely
reuse one type in both layers and accept the coupling; Rust's orphan rules
and explicit conversions nudge you toward keeping boundaries real.

## 3. Grouping with `entry().or_insert_with()` and function-local structs

**Where:** `src/aggregate.rs::sessions` and `::projects`.

The dedupe module's entry-API pattern reappears for grouping: get-or-create
a bucket in one hash lookup, then mutate it in place (`session.start =
session.start.min(event.timestamp)`). Note `projects` defines `struct
Bucket {...}` *inside the function* — a type that exists only for one
computation doesn't deserve module scope. Rust items can nest almost
anywhere, which keeps helper types exactly as private as they should be.

## 4. Const generics for a tiny ergonomic win

**Where:** `src/report/table.rs::new_table<const N: usize>([&str; N])`.

Every table has a different column count, and `new_table` accepts a
fixed-size array of *any* length — `[&str; 6]` and `[&str; 8]` are
different types, and `const N: usize` makes one function serve both.
The Swift analogue is variadic generics (much newer); the Go analogue is
`[]string` and losing the compile-time length. Here the length isn't
load-bearing — it's just nicer than `vec![]` at every call site — but the
mechanism scales up to serious compile-time guarantees.

## 5. Process discipline note: refactor-under-green

The rayon change (item 1) added no behavior, so it got **no new tests** —
the existing 90 stayed green through it, and the benchmark, not a unit
test, proves the point of the change. Distinguishing "new behavior needs
red-first" from "performance refactor needs green-throughout" is the TDD
judgment call this phase exercised most.

## Borrow-checker moments

None forced a redesign this phase. The nearest miss: rayon's `map` closure
moves each `TranscriptFile` in and returns it as part of the tuple —
returning ownership through the pipeline instead of borrowing across
threads is what made the parallel version compile on the first try.
