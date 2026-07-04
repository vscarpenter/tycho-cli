# Phase 1 — Rust concepts this phase exercised

Five concepts, each pointing at the real code. Read them next to the files.

## 1. `Option` as boundary honesty (vs Go's zero values)

**Where:** `src/record.rs` — `RawRecord` (every field `Option`) vs `UsageEvent` (no `Option` except `session_id`).

In Go, a missing JSON field silently becomes `""` or `0`, and you can't
tell "absent" from "actually zero" without pointer fields or `ok` flags. In
Rust, absence is a *type*: `Option<String>` forces every consumer to decide
what missing means. The pattern this codebase uses is **permissive edge,
strict core**: `RawRecord` admits anything (all `Option`), and
`parse_line` is the single checkpoint that converts it into a `UsageEvent`
whose fields downstream code can use without ever re-checking. Once you
hold a `UsageEvent`, `event.model` just *is* a `String` — the compiler
remembers the validation so you don't have to.

## 2. Errors as data: `Result<UsageEvent, SkipReason>`

**Where:** `src/record.rs::parse_line`, consumed in `src/scan.rs::scan`.

`SkipReason` is a plain enum, not an "exception": a malformed line is a
normal outcome for this tool, so it flows through the same `Result` channel
as success and gets *counted*, not thrown. Each `?`/`ok_or` line in
`parse_line` reads as a validation gate. Contrast the binary: `main.rs`
uses `anyhow` because at the process boundary all errors collapse into
"print a message, exit 1" — rich error taxonomies only matter where callers
can react to them. That's the library-`thiserror`/binary-`anyhow` split in
its simplest form (Phase 1 hasn't even needed `thiserror` yet; `SkipReason`
is data, not error).

## 3. The `HashMap` entry API and the borrow checker

**Where:** `src/dedupe.rs::Deduper::insert`.

`entry()` does one hash lookup and returns a *view* (`Vacant`/`Occupied`)
you decide with — Go's read-then-write `m[k]` pattern does two lookups and
says nothing about the in-between. The borrow-checker moment: inside
`Occupied`, `slot.get()` borrows the kept event, and that borrow must end
before `slot.insert(event)` can take ownership. Order the lines wrong and
it does not compile. Bonus one-liner: "max output, tie-break latest
timestamp" is a lexicographic tuple comparison —
`(a.output, a.timestamp) > (b.output, b.timestamp)`.

## 4. The first lifetime: `EventFilter<'a>` (vs Swift's ARC)

**Where:** `src/scan.rs` — `pub struct EventFilter<'a> { project: Option<&'a str>, ... }`.

The `'a` says: *this filter may not outlive the strings it borrows from*
(the CLI args in `main.rs`). Lifetimes change nothing at runtime; they are
compile-time proofs about reference validity. In Swift you'd pass `String`
and let ARC reference-count it; in Go the GC hides the question entirely.
Rust makes the choice explicit, and the idiom is: **borrow for read-only
views, own for data you keep**. `UsageEvent` *owns* its `String`s because
it outlives the parse; `EventFilter` *borrows* because it lives only for
one scan. When a lifetime fights you, owning (`String`) is always the
correct-first escape hatch.

## 5. Iterator pipelines (vs index loops), and why they're free

**Where:** `src/discover.rs::discover` (`flat_map → filter → map → collect`),
`src/aggregate.rs::daily` (BTreeMap for free sorted output).

Rust iterators are lazy and fused: the chain compiles to the same machine
code as a hand-written nested loop — no per-stage allocations, nothing
happens until `collect()`. The win is that each stage names its intent
(`filter_map(Result::ok)` = "keep the Oks, drop the Errs"). Related
pick: `aggregate::daily` buckets into a `BTreeMap` instead of `HashMap`
because B-tree iteration order *is* date order — the sort disappears into
the data structure choice.

## Borrow-checker-forced designs this phase

Nothing required a redesign; two places required a specific *order*:
the `Occupied` comparison in `dedupe.rs` (item 3), and `parse_line`'s
partial moves (fields are moved out of `raw` one at a time, so reads of a
field must precede the move of its sibling struct). Both compile errors
were of the "you wrote it in the wrong order" kind, not the "your
architecture is wrong" kind — typical of code with clear ownership.

## Edition-2024 niceties used

- **Let-chains** (`if let Some(p) = filter.project && !file.project.contains(p)`)
  in `scan.rs` — flattened what used to be two nesting levels (needs Rust 1.88+).
- `is_some_and` / `is_multiple_of` — small stdlib predicates clippy now
  steers you toward.
