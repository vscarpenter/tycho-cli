# Phase 3 — Rust concepts this phase exercised

## 1. The thiserror / anyhow split, finally in action

**Where:** `src/pricing.rs::PricingError` (thiserror) vs `src/main.rs::resolve_pricing` (anyhow).

Phases 1–2 never needed a custom error type — skips were data, I/O errors
were counted. Pricing is the first place a *caller can react*: a bad
`--pricing` file is a typed `PricingError` with `#[from]` conversions from
`toml::de::Error` and `std::io::Error` (the `?` operator does the wrapping).
The binary then uses anyhow's `.with_context(|| format!("loading {path}"))`
to decorate it for humans. That's the whole doctrine in one hop: libraries
return types callers can match on; binaries flatten everything into one
printable chain. Go comparison: `fmt.Errorf("loading %s: %w", path, err)`
does both jobs with one mechanism — Rust splits the two audiences into two
crates, and the compiler enforces which side you're on.

## 2. `Decimal` for ledgers, floats for graphs

**Where:** `src/cost.rs::calculate`, `src/pricing.rs` (serde-float), `src/report/json.rs` (`to_f64` at the edge).

`0.1 + 0.2 != 0.3` in f64, and a cost report that drifts by fractions of a
cent invites distrust. Every internal money value is `rust_decimal::Decimal`
— exact base-10 arithmetic, `Copy`, and divides by 1,000,000 exactly (a
power of ten just shifts the scale). Floats appear only at the two borders:
TOML rates parse via the `serde-float` feature, and JSON output converts
with `to_f64()` because JSON numbers are floats anyway. The verification
payoff was visible at the gate: the jq oracle (f64) computed
`3420.6745642999927`; tycho printed `3420.6745643` — the noise is the
float's, not ours.

## 3. `Option` as "this math is undefined"

**Where:** `src/cache.rs::CacheEconomics` — `hit_rate: Option<Decimal>`, `leverage: Option<Decimal>`.

The spec says "guard against divide-by-zero." The C-family reflex is a
sentinel (0.0? -1? NaN?), which every renderer must remember to check. Here
the type says it: a window with no cacheable input has *no* hit rate, not a
zero one, so `hit_rate` is `None` and the compiler forces `table.rs` and
`json.rs` to decide what "undefined" renders as (`-` and `null`). The guard
lives in exactly one place — `(cacheable > 0).then(|| ...)` — and can't be
forgotten downstream.

## 4. A second lifetime: `Coster<'a>` borrows the table

**Where:** `src/cost.rs::Coster<'a>` holding `table: &'a PricingTable`.

Phase 1's `EventFilter<'a>` borrowed strings; `Coster<'a>` borrows a whole
struct. Same rule, bigger object: the coster is a short-lived *view* used
during one run, so it borrows rather than clones the table. Note what the
compiler now guarantees: the pricing table cannot be dropped or mutated
while any coster exists. In Swift, ARC would keep the table alive but
happily let another thread mutate it; here the aliasing rules make
"shared + immutable" a compile-time property.

## 5. `include_str!` — data compiled into the binary

**Where:** `src/pricing.rs::DEFAULT_TOML`.

`include_str!("../pricing/default.toml")` pastes the file into the binary
as a `&'static str` at compile time. No install step, no data files to
locate at runtime, no I/O to fail — which is what lets `PricingTable::
embedded()` avoid panicking APIs entirely: a unit test parses the embedded
text, so corruption fails CI, not the user's terminal. This is a very
Rust-flavored answer to Go's `//go:embed` (nearly identical) and has no
clean Swift analog.

## Borrow-checker moments

One genuine fight this phase: the first draft threaded `&Coster` into every
aggregate function so buckets could price events as they were folded in —
five signatures deep, lifetimes spreading through the report types. The fix
was to *stamp* the cost onto each event up front (`Coster::apply`) and let
`Totals::add` sum a plain field. Less clever, fewer lifetimes, and the
aggregators stayed pricing-agnostic. When borrows start propagating through
signatures that don't care about them, the idiomatic move is often to make
the data own what it needs.
