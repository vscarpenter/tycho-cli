# Cost Truthfulness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recover Codex model attribution lost on resumed session files, surface the 5m/1h cache-write TTL split, and report a shadow cost estimate for deliberately unpriced models.

**Architecture:** Three independent changes to existing modules. The backfill is a per-file post-pass inside `record::parse_file`, counted in `ParseStats`. The TTL split extends `cache::CacheEconomics` and the two `cache` renderers. Shadow estimates add an optional `[shadow]` section to `PricingTable` plus two functions in `cost.rs`, surfaced only by `doctor`.

**Tech Stack:** Rust 1.88+ stable, `rust_decimal` for money, `serde`/`toml` for pricing, `rayon` for the parallel scan, `comfy-table` for table output, `insta`-free hand-rolled JSON contract tests.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-09-cost-truthfulness-design.md`
- Money is `Decimal`, never `f64`. Rates are USD per million tokens.
- TDD: write the failing test, watch it fail for the right reason, then implement.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must be green at every commit.
- Never verify through a pipe without `set -o pipefail` (see `tasks/lessons.md`).
- JSON changes are additive only; existing keys keep their names and types.
- The shadow estimate is a diagnostic. It never enters `cost`, never changes a report total, and appears in no report but `doctor`.
- Commit each task separately using the `creating-git-commits` skill conventions (Conventional Commit with scope, `Claude-Session:` trailer, no `Co-Authored-By: Claude`).

---

### Task 1: Codex model backfill

**Files:**
- Modify: `src/record.rs` (add `CODEX_UNKNOWN` const, `first_codex_model` parser field, `codex_model_backfilled` stat, `backfill_codex_model` fn, wire into `parse_file`)
- Modify: `src/report/table.rs` (doctor row)
- Modify: `src/report/json.rs` (doctor field)
- Modify: `docs/SCHEMA.md` (describe the backfill)
- Modify: `README.md` (accuracy caveat about the changed historical totals)
- Test: `src/record.rs` `mod tests` (unit), `src/report/json.rs` `mod tests` (contract)

**Interfaces:**
- Consumes: `FileScan { events: Vec<UsageEvent>, stats: ParseStats }`, `LineParser`, `parse_file(path: &Path, provider: Provider) -> io::Result<FileScan>`
- Produces: `ParseStats::codex_model_backfilled: u64`, merged by `ParseStats::merge`, reaching `doctor` via `ScanSummary::stats`

- [ ] **Step 1: Write the failing test**

Add to `src/record.rs` `mod tests`:

```rust
/// A resumed Codex rollout whose context lines were trimmed: the first
/// token_count precedes any turn_context. The model is still recoverable
/// from the turn_context that appears later in the same file.
#[test]
fn codex_token_counts_before_turn_context_are_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-resumed.jsonl");
    let usage = r#""info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":7,"total_tokens":107}}"#;
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n",
            format!(r#"{{"type":"event_msg","timestamp":"2026-07-08T01:00:00Z","payload":{{"type":"token_count",{usage}}}}}"#),
            r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:01Z","payload":{"model":"gpt-5.6-sol","cwd":"/Users/v/Projects/gsd"}}"#,
            format!(r#"{{"type":"event_msg","timestamp":"2026-07-08T01:00:02Z","payload":{{"type":"token_count",{usage}}}}}"#),
        ),
    )
    .unwrap();

    let scan = parse_file(&path, Provider::Codex).unwrap();
    let models: Vec<_> = scan.events.iter().map(|e| e.model.as_str()).collect();
    assert_eq!(models, ["gpt-5.6-sol", "gpt-5.6-sol"]);
    assert_eq!(scan.stats.codex_model_backfilled, 1);
}

/// Nothing to recover: the file never names a model, so the placeholder
/// stands and nothing is counted as backfilled.
#[test]
fn codex_files_with_no_turn_context_keep_the_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-old.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"event_msg\",\"timestamp\":\"2025-09-11T01:00:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":2,\"total_tokens\":12}}}}\n",
    )
    .unwrap();

    let scan = parse_file(&path, Provider::Codex).unwrap();
    assert_eq!(scan.events[0].model, CODEX_UNKNOWN);
    assert_eq!(scan.stats.codex_model_backfilled, 0);
}

/// When a file switches models, events that preceded the FIRST turn_context
/// belong to that first model, not to whichever ran last.
#[test]
fn backfill_uses_the_first_model_not_the_last() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-switch.jsonl");
    let usage = r#""info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":2,"total_tokens":12}}"#;
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n",
            format!(r#"{{"type":"event_msg","timestamp":"2026-07-08T01:00:00Z","payload":{{"type":"token_count",{usage}}}}}"#),
            r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:01Z","payload":{"model":"gpt-5.6-sol"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:02Z","payload":{"model":"gpt-5.5"}}"#,
        ),
    )
    .unwrap();

    let scan = parse_file(&path, Provider::Codex).unwrap();
    assert_eq!(scan.events[0].model, "gpt-5.6-sol");
}

/// Backfill is per file. A file that names a model must not lend it to a
/// different file parsed in the same scan.
#[test]
fn backfill_does_not_leak_between_files() {
    let dir = tempfile::tempdir().unwrap();
    let usage = r#""info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":2,"total_tokens":12}}"#;
    let tc = r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:01Z","payload":{"model":"gpt-5.6-sol"}}"#;
    let counted =
        format!(r#"{{"type":"event_msg","timestamp":"2026-07-08T01:00:00Z","payload":{{"type":"token_count",{usage}}}}}"#);

    let named = dir.path().join("named.jsonl");
    std::fs::write(&named, format!("{counted}\n{tc}\n")).unwrap();
    let bare = dir.path().join("bare.jsonl");
    std::fs::write(&bare, format!("{counted}\n")).unwrap();

    assert_eq!(
        parse_file(&named, Provider::Codex).unwrap().events[0].model,
        "gpt-5.6-sol"
    );
    assert_eq!(
        parse_file(&bare, Provider::Codex).unwrap().events[0].model,
        CODEX_UNKNOWN
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib record:: 2>&1 | tail -20`
Expected: FAIL — `cannot find value CODEX_UNKNOWN in this scope` and `no field codex_model_backfilled on type ParseStats`.

- [ ] **Step 3: Add the constant and the stat field**

In `src/record.rs`, above `struct LineParser`:

```rust
/// Placeholder model id for Codex `token_count` records reached before the
/// file's first `turn_context` supplied a real one.
pub const CODEX_UNKNOWN: &str = "codex-unknown";
```

In `ParseStats`, after `synthetic`:

```rust
    /// Codex events whose `codex-unknown` placeholder was replaced by the
    /// file's first known model (see `backfill_codex_model`).
    pub codex_model_backfilled: u64,
```

In `ParseStats::merge`, after the `synthetic` line:

```rust
        self.codex_model_backfilled += other.codex_model_backfilled;
```

- [ ] **Step 4: Track the first model in the parser**

In `struct LineParser`, after `codex_model`:

```rust
    /// The first model any `turn_context` in this file supplied. Distinct
    /// from `codex_model`, which tracks the *current* model and would hold
    /// the last value at end of file.
    first_codex_model: Option<String>,
```

In `impl Default for LineParser`, after `codex_model: None,`:

```rust
            first_codex_model: None,
```

In the `Some("turn_context")` arm, replace the model assignment with:

```rust
                if let Some(model) = payload.model.as_ref() {
                    self.codex_model = Some(model.clone());
                    if self.first_codex_model.is_none() {
                        self.first_codex_model = Some(model.clone());
                    }
                }
```

Replace the literal at the `codex-unknown` fallback with the constant:

```rust
            .unwrap_or_else(|| CODEX_UNKNOWN.to_owned());
```

- [ ] **Step 5: Implement the backfill and wire it into `parse_file`**

Add above `parse_file`:

```rust
/// Replace the `codex-unknown` placeholder left on events that preceded the
/// file's first `turn_context`.
///
/// `codex_model` is set once and never reverts to `None`, so an event can
/// only carry the placeholder if it was parsed before that record. The
/// file's first known model is therefore the correct value for every one of
/// them, and no per-event search is needed. Backfill is per file: the
/// parser state that produced `first_model` never crosses a file boundary.
fn backfill_codex_model(scan: &mut FileScan, first_model: Option<&str>) {
    let Some(model) = first_model else { return };
    let FileScan { events, stats } = scan;
    for event in events.iter_mut() {
        if event.model == CODEX_UNKNOWN {
            event.model.clear();
            event.model.push_str(model);
            stats.codex_model_backfilled += 1;
        }
    }
}
```

In `parse_file`, change the end-of-file return:

```rust
        if reader.read_until(b'\n', &mut buf)? == 0 {
            backfill_codex_model(&mut scan, parser.first_codex_model.as_deref());
            return Ok(scan);
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `set -o pipefail; cargo test --lib record:: 2>&1 | tail -20`
Expected: PASS, including the three new tests.

- [ ] **Step 7: Surface the counter in `doctor`**

`table::doctor` builds its rows as a fixed-size array. Widen the type
annotation from `[(&str, String); 14]` to `[(&str, String); 15]` and insert
this entry between `Synthetic (API error) records` and `Date span`:

```rust
        (
            "Codex models backfilled",
            group_thousands(stats.codex_model_backfilled),
        ),
```

Note `group_thousands` is the formatter every neighbouring row uses, and
`stats` is already bound to `&report.summary.stats` above the array.

In `src/report/json.rs`, add to the doctor serialization struct beside the other stat fields:

```rust
    codex_model_backfilled: u64,
```

and populate it:

```rust
        codex_model_backfilled: report.summary.stats.codex_model_backfilled,
```

Extend the existing `doctor_contract` test with:

```rust
        assert_eq!(value["codex_model_backfilled"], 0);
```

- [ ] **Step 8: Verify the whole suite and lints**

Run: `set -o pipefail; cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -20`
Expected: all green.

- [ ] **Step 9: Document the behaviour change**

In `docs/SCHEMA.md`, in the paragraph that introduces `codex-unknown`, append:

```markdown
When a later `turn_context` in the same file does supply a model, tycho
backfills it onto those events after the file finishes parsing, and counts
the correction as `Codex models backfilled` in `doctor`. Because
`codex_model` is set once and never reverts, the placeholder can only appear
before the file's first `turn_context`, so that first model is the correct
value for every placeholder in the file. Files that never name a model keep
`codex-unknown`.
```

In `README.md`, under the accuracy caveats section, add:

```markdown
- Codex rollouts whose context lines were trimmed (resumed sessions) record
  usage before naming their model. tycho recovers the model from the file's
  first `turn_context` and reports the count as `Codex models backfilled` in
  `tycho doctor`. This lands spend that earlier versions reported at $0, so
  historical totals can increase after upgrading.
```

- [ ] **Step 10: Commit**

```bash
git add src/record.rs src/report/table.rs src/report/json.rs docs/SCHEMA.md README.md
git commit   # message per creating-git-commits: fix(record): backfill Codex model from the file's first turn_context
```

---

### Task 2: Cache TTL split on `tycho cache`

**Files:**
- Modify: `src/cache.rs` (`CacheEconomics::ttl_premium`, `economics`, `with_counterfactual`, total)
- Modify: `src/report/table.rs` (narrative sentence, `Write 5m` / `Write 1h` columns)
- Modify: `src/report/json.rs` (three additive fields)
- Modify: `README.md` (updated `tycho cache` sample output)
- Test: `src/cache.rs` `mod tests`, `src/report/json.rs` `mod tests`

**Interfaces:**
- Consumes: `Totals { cache_write_5m, cache_write_1h, .. }`, `ModelPricing { cache_write_5m, cache_write_1h, .. }`, `PricingTable::lookup`
- Produces: `CacheEconomics::ttl_premium: Decimal`, summed on the `Total` row like `counterfactual_cost`

- [ ] **Step 1: Write the failing test**

Add to `src/cache.rs` `mod tests`:

```rust
/// The premium is what the 1-hour TTL cost over the 5-minute rate for the
/// same tokens: 2_000_000 * (20.0 - 12.5) / 1e6 = 15.
#[test]
fn ttl_premium_prices_1h_writes_against_the_5m_rate() {
    let usage = TokenUsage {
        input: 0,
        output: 0,
        cache_write_5m: 1_000_000,
        cache_write_1h: 2_000_000,
        cache_read: 0,
    };
    let report = cache(
        [event("claude-fable-5", usage, "0")],
        chrono_tz::UTC,
        None,
        None,
        &PricingTable::embedded(),
    );
    assert_eq!(report.models[0].ttl_premium, dec("15"));
    assert_eq!(report.total.ttl_premium, dec("15"));
}

/// A model with no 1-hour writes owes no premium.
#[test]
fn ttl_premium_is_zero_without_1h_writes() {
    let usage = TokenUsage {
        input: 0,
        output: 0,
        cache_write_5m: 5_000_000,
        cache_write_1h: 0,
        cache_read: 0,
    };
    let report = cache(
        [event("claude-fable-5", usage, "0")],
        chrono_tz::UTC,
        None,
        None,
        &PricingTable::embedded(),
    );
    assert_eq!(report.models[0].ttl_premium, Decimal::ZERO);
}

/// Unpriced models contribute no premium rather than panicking.
#[test]
fn ttl_premium_is_zero_for_unpriced_models() {
    let usage = TokenUsage {
        input: 0,
        output: 0,
        cache_write_5m: 0,
        cache_write_1h: 1_000_000,
        cache_read: 0,
    };
    let report = cache(
        [event("qwen3.6:27b", usage, "0")],
        chrono_tz::UTC,
        None,
        None,
        &PricingTable::embedded(),
    );
    assert_eq!(report.models[0].ttl_premium, Decimal::ZERO);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `set -o pipefail; cargo test --lib cache:: 2>&1 | tail -20`
Expected: FAIL — `no field ttl_premium on type CacheEconomics`.

- [ ] **Step 3: Add the field and compute it**

In `src/cache.rs`, add to `CacheEconomics` after `leverage`:

```rust
    /// Extra cost incurred because cache writes used the 1-hour TTL instead
    /// of the 5-minute one: `cache_write_1h * (rate_1h - rate_5m)`. Zero for
    /// models with no rates.
    pub ttl_premium: Decimal,
```

Rewrite `economics` to compute both figures from a single lookup:

```rust
fn economics(model: String, totals: Totals, table: &PricingTable) -> CacheEconomics {
    let million = Decimal::from(1_000_000u32);
    let rates = table.lookup(&model);
    let counterfactual = rates
        .map(|rates| {
            let all_input =
                totals.input + totals.cache_write_5m + totals.cache_write_1h + totals.cache_read;
            Decimal::from(all_input) * rates.input / million
                + Decimal::from(totals.output) * rates.output / million
        })
        .unwrap_or(Decimal::ZERO);
    let ttl_premium = rates
        .map(|rates| {
            Decimal::from(totals.cache_write_1h) * (rates.cache_write_1h - rates.cache_write_5m)
                / million
        })
        .unwrap_or(Decimal::ZERO);
    with_counterfactual(model, totals, counterfactual, ttl_premium)
}
```

Change `with_counterfactual`'s signature and struct literal:

```rust
fn with_counterfactual(
    model: String,
    totals: Totals,
    counterfactual: Decimal,
    ttl_premium: Decimal,
) -> CacheEconomics {
```

and add `ttl_premium,` to the returned `CacheEconomics`.

In `cache`, sum the premium for the total row exactly as the counterfactual is summed:

```rust
    let counterfactual = models.iter().map(|m| m.counterfactual_cost).sum();
    let ttl_premium = models.iter().map(|m| m.ttl_premium).sum();
    let total = with_counterfactual("Total".to_owned(), by_model.total, counterfactual, ttl_premium);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `set -o pipefail; cargo test --lib cache:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Write the failing renderer tests**

Add to `src/report/table.rs` `mod tests`:

```rust
/// Build a one-model cache report with the given write split.
fn ttl_report(write_5m: u64, write_1h: u64, premium: &str) -> crate::cache::CacheReport {
    let totals = Totals {
        cache_write_5m: write_5m,
        cache_write_1h: write_1h,
        cache_read: 1_000,
        ..Totals::default()
    };
    let row = crate::cache::CacheEconomics {
        model: "claude-fable-5".to_owned(),
        totals,
        hit_rate: None,
        actual_cost: rust_decimal::Decimal::ONE,
        counterfactual_cost: rust_decimal::Decimal::TWO,
        savings: rust_decimal::Decimal::ONE,
        leverage: Some(rust_decimal::Decimal::TWO),
        ttl_premium: premium.parse().unwrap(),
    };
    crate::cache::CacheReport {
        models: vec![row.clone()],
        total: crate::cache::CacheEconomics {
            model: "Total".to_owned(),
            ..row
        },
    }
}

/// The single write column becomes two, and the narrative reports the
/// 1-hour share and what it cost.
#[test]
fn cache_table_splits_write_columns_and_reports_the_ttl_premium() {
    let out = cache(&ttl_report(1_000_000, 3_000_000, "22.5"), false);
    assert!(out.contains("Write 5m"));
    assert!(out.contains("Write 1h"));
    assert!(!out.contains("Cache Write"));
    assert!(out.contains("75.0% of your cache writes used the 1-hour TTL"));
    assert!(out.contains("$22.50"));
}

/// With no 1-hour writes there is no premium to report, so the sentence is
/// omitted entirely rather than printing a $0.00 line.
#[test]
fn cache_table_omits_the_ttl_sentence_without_1h_writes() {
    let out = cache(&ttl_report(1_000_000, 0, "0"), false);
    assert!(!out.contains("1-hour TTL"));
}
```

- [ ] **Step 6: Run it to verify it fails**

Run: `set -o pipefail; cargo test --lib report::table 2>&1 | tail -20`
Expected: FAIL — output still contains `Cache Write` and no `1-hour TTL`.

- [ ] **Step 7: Update the renderer**

In `src/report/table.rs::cache` (line 194), make `headline` mutable and append
the TTL sentence to it. The `cache_write_1h > 0` guard also makes the divisor
non-zero, so no separate zero check is needed:

```rust
    let mut headline = match total.leverage {
        Some(leverage) => format!(
            "Your effective cost was {}. Without prompt caching it would have been {}.\nCaching saved you {} ({} leverage).\n",
            money(total.actual_cost, precise),
            money(total.counterfactual_cost, precise),
            money(total.savings, precise),
            leverage_x(Some(leverage)),
        ),
        None => "No costed usage in this window.\n".to_owned(),
    };
    if total.totals.cache_write_1h > 0 {
        let writes = rust_decimal::Decimal::from(cache_write(&total.totals));
        let share = rust_decimal::Decimal::from(total.totals.cache_write_1h)
            * rust_decimal::Decimal::from(100u8)
            / writes;
        headline.push_str(&format!(
            "{}% of your cache writes used the 1-hour TTL, costing {} more than the 5-minute rate.\n",
            share.round_dp_with_strategy(1, rust_decimal::RoundingStrategy::MidpointAwayFromZero),
            money(total.ttl_premium, precise),
        ));
    }
```

Then split the column. In the `new_table([...])` call replace `"Cache Write"`
with `"Write 5m", "Write 1h"`, and in the row body replace
`group_thousands(cache_write(&row.totals)),` with:

```rust
            group_thousands(row.totals.cache_write_5m),
            group_thousands(row.totals.cache_write_1h),
```

Leave the `cache_write` helper in place — `totals_row` still uses it at
line 52 for the daily/monthly tables, so it does not become dead code.

- [ ] **Step 8: Run the renderer test to verify it passes**

Run: `set -o pipefail; cargo test --lib report::table 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 9: Extend the JSON contract**

In `src/report/json.rs`, add to the per-model and total cache serialization structs:

```rust
    cache_write_5m: u64,
    cache_write_1h: u64,
    ttl_premium_usd: String,
```

Populate from `row.totals.cache_write_5m`, `row.totals.cache_write_1h`, and `row.ttl_premium` (formatted with the same Decimal-to-string helper the neighbouring money fields use).

Extend the existing cache contract test:

```rust
        assert_eq!(value["models"][0]["cache_write_1h"], 2_000_000);
        assert_eq!(value["total"]["ttl_premium_usd"], "15.00");
```

- [ ] **Step 10: Verify everything and refresh the README sample**

Run: `set -o pipefail; cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -20`

Then regenerate the `tycho cache` block in `README.md` from a real run so the documented output matches the new columns and narrative.

- [ ] **Step 11: Commit**

```bash
git add src/cache.rs src/report/table.rs src/report/json.rs README.md
git commit   # feat(cache): split cache writes by TTL and report the 1-hour premium
```

---

### Task 3: Shadow estimates in `tycho doctor`

**Files:**
- Modify: `src/pricing.rs` (`shadow` map, `shadow_reference`, shared prefix matcher, `merge`, `validate`, new error variant)
- Modify: `pricing/default.toml` (`[shadow]` section + comment)
- Modify: `src/cost.rs` (`unpriced_token_total`, `shadow_estimate`, `is_unpriced`)
- Modify: `src/scan.rs` (`DoctorReport` fields), `src/main.rs` (wire + validate)
- Modify: `src/report/table.rs`, `src/report/json.rs` (two rows / three fields)
- Test: `src/pricing.rs`, `src/cost.rs`, `src/report/json.rs` `mod tests`

**Interfaces:**
- Consumes: `PricingTable::lookup`, `cost::all_rates_are_zero`, `UsageEvent::{model, usage}`
- Produces: `PricingTable::shadow_reference(&self, model: &str) -> Option<&str>`, `PricingTable::validate(&self) -> Result<(), PricingError>`, `cost::ShadowDiagnostic { tokens: u64, models: Vec<String>, estimate: Decimal, mappings: BTreeMap<String, String> }`, `cost::shadow_diagnostic(&[UsageEvent], &PricingTable) -> ShadowDiagnostic`, `DoctorReport::{unpriced_tokens: u64, unpriced_model_count: usize, shadow_estimate: Decimal, shadow_mappings: BTreeMap<String, String>}`

**Design note:** one function returning one struct, rather than three parallel
scans of the same events. `doctor` needs the token count, the model count, the
dollar estimate, and the mapping table together; computing them in one pass
keeps them consistent by construction.

- [ ] **Step 1: Write the failing pricing tests**

Add to `src/pricing.rs` `mod tests`:

```rust
#[test]
fn shadow_reference_resolves_by_the_same_prefix_rule_as_lookup() {
    let table = PricingTable::embedded();
    assert_eq!(table.shadow_reference("gpt-5-codex"), Some("gpt-5.4"));
    // '-' boundary applies here too: the -max variant inherits its base.
    assert_eq!(table.shadow_reference("gpt-5.1-codex-max"), Some("gpt-5.4"));
    // Priced models need no stand-in.
    assert_eq!(table.shadow_reference("claude-opus-5"), None);
}

#[test]
fn shadow_value_naming_an_unpriced_model_fails_validation() {
    let table = PricingTable::parse(
        r#"
        [models."real-model"]
        input = 1.0
        output = 1.0
        cache_write_5m = 1.0
        cache_write_1h = 1.0
        cache_read = 1.0

        [shadow]
        "legacy" = "does-not-exist"
        "#,
    )
    .unwrap();
    let err = table.validate().unwrap_err();
    assert!(matches!(err, PricingError::UnknownShadowReference { .. }));
}

#[test]
fn embedded_table_validates() {
    PricingTable::embedded().validate().unwrap();
}

#[test]
fn merge_carries_shadow_mappings() {
    let mut base = PricingTable::embedded();
    base.merge(
        PricingTable::parse(
            r#"
            [shadow]
            "gpt-5-codex" = "gpt-5.5"
            "#,
        )
        .unwrap(),
    );
    assert_eq!(base.shadow_reference("gpt-5-codex"), Some("gpt-5.5"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `set -o pipefail; cargo test --lib pricing:: 2>&1 | tail -20`
Expected: FAIL — `no method named shadow_reference`.

- [ ] **Step 3: Implement the shadow map**

In `src/pricing.rs`, extract the prefix rule so `lookup` and `shadow_reference` cannot drift:

```rust
/// Longest-prefix match with a `-` boundary, shared by the rate and shadow
/// maps so the two can never disagree about what an id resolves to.
fn longest_prefix_match<'a, V>(map: &'a BTreeMap<String, V>, model: &str) -> Option<&'a V> {
    map.iter()
        .filter(|(key, _)| {
            model == key.as_str()
                || (model.starts_with(key.as_str())
                    && model.as_bytes().get(key.len()) == Some(&b'-'))
        })
        .max_by_key(|(key, _)| key.len())
        .map(|(_, value)| value)
}
```

Add the field to `PricingTable`:

```rust
    /// Optional stand-in rates for models that are deliberately zero-rated
    /// or absent: observed model id -> the id whose rates represent it.
    /// Used only by `doctor`'s shadow estimate, never by the cost engine.
    #[serde(default)]
    shadow: BTreeMap<String, String>,
```

Rewrite `lookup` as `longest_prefix_match(&self.models, model)` and add:

```rust
    /// The reference model whose rates stand in for `model`, if the table
    /// defines one.
    pub fn shadow_reference(&self, model: &str) -> Option<&str> {
        longest_prefix_match(&self.shadow, model).map(String::as_str)
    }

    /// Check that every `[shadow]` value names a model this table can price.
    /// Run after merging user overrides, since a user table may legitimately
    /// reference a model defined only in the embedded defaults.
    pub fn validate(&self) -> Result<(), PricingError> {
        for (model, reference) in &self.shadow {
            if self.lookup(reference).is_none() {
                return Err(PricingError::UnknownShadowReference {
                    model: model.clone(),
                    reference: reference.clone(),
                });
            }
        }
        Ok(())
    }
```

Extend `merge`:

```rust
        self.shadow.extend(overrides.shadow);
```

Add the error variant:

```rust
    /// A `[shadow]` entry names a model with no pricing entry.
    #[error("shadow mapping for {model:?} names {reference:?}, which has no pricing entry")]
    UnknownShadowReference { model: String, reference: String },
```

In `pricing/default.toml`, extend the header comment and add the section:

```toml
# The [shadow] section maps deliberately zero-rated or unpriced model ids to
# a reference model whose rates stand in for them. It feeds `tycho doctor`'s
# shadow estimate only — it never changes any reported cost. Override or
# extend it exactly like [models].

[shadow]
"gpt-5-codex" = "gpt-5.4"
"gpt-5.1-codex" = "gpt-5.4"
"gpt-5.2-codex" = "gpt-5.4"
```

- [ ] **Step 4: Run to verify the pricing tests pass**

Run: `set -o pipefail; cargo test --lib pricing:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Write the failing cost tests**

Add to `src/cost.rs` `mod tests`:

```rust
/// Local name:tag models cost nothing as a fact, not as a gap, so they are
/// excluded from the unpriced tally. Priced models are excluded too.
#[test]
fn unpriced_tally_counts_only_models_with_no_real_rates() {
    let table = PricingTable::embedded();
    let events = [
        event("qwen3.6:27b", 100, 10),   // local: excluded
        event("gpt-5-codex", 200, 20),   // zero-rated: counted
        event("claude-opus-5", 400, 40), // priced: excluded
    ];
    let diag = shadow_diagnostic(&events, &table);
    assert_eq!(diag.tokens, 220);
    assert_eq!(diag.models, ["gpt-5-codex"]);
}

/// A zero-rated model with a [shadow] entry contributes dollars; one
/// without contributes only tokens. gpt-5.4 input is $5/Mtok, so one
/// million input tokens is exactly $5.
#[test]
fn shadow_estimate_prices_only_mapped_models() {
    let table = PricingTable::embedded();
    let mapped = shadow_diagnostic(&[event("gpt-5-codex", 1_000_000, 0)], &table);
    assert_eq!(mapped.estimate, dec("5"));
    assert_eq!(mapped.mappings["gpt-5-codex"], "gpt-5.4");

    // codex-unknown has no [shadow] entry by design: its defining property
    // is that the model is unknown, so no reference rate can be justified.
    let unmapped = shadow_diagnostic(&[event("codex-unknown", 1_000_000, 0)], &table);
    assert_eq!(unmapped.estimate, Decimal::ZERO);
    assert_eq!(unmapped.tokens, 1_000_000);
    assert!(unmapped.mappings.is_empty());
}
```

Define the local helper beside the module's existing test helpers:

```rust
fn event(model: &str, input: u64, output: u64) -> UsageEvent {
    UsageEvent {
        timestamp: "2026-07-02T10:00:00Z".parse().unwrap(),
        session_id: None,
        project: String::new(),
        model: model.to_owned(),
        usage: TokenUsage {
            input,
            output,
            ..TokenUsage::default()
        },
        cost_usd: None,
        cost: Decimal::ZERO,
        dedup_key: DedupKey::Uuid(model.to_owned()),
        provider: Provider::Codex,
    }
}
```

- [ ] **Step 6: Run to verify failure**

Run: `set -o pipefail; cargo test --lib cost:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function shadow_diagnostic`.

- [ ] **Step 7: Implement the diagnostic**

In `src/cost.rs`:

```rust
/// What `doctor` reports about tokens that carry no real rates: how many
/// there are, which models they came from, what they would cost at
/// `[shadow]` reference rates, and which reference each model used.
///
/// Diagnostic only. Nothing here enters `Coster`, and no report total
/// includes `estimate`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShadowDiagnostic {
    /// Total tokens across models with no real rates.
    pub tokens: u64,
    /// Those models' ids, sorted and deduplicated.
    pub models: Vec<String>,
    /// Cost at reference rates. Zero when nothing is mapped.
    pub estimate: Decimal,
    /// Observed model -> the reference model standing in for it.
    pub mappings: BTreeMap<String, String>,
}

/// Whether a model contributes nothing to cost because it has no real rates
/// — a zero-rated entry, or no entry at all. Local Ollama-style `name:tag`
/// models are excluded: their $0 is a fact about local inference, not a gap
/// in the pricing table.
fn is_unpriced(model: &str, table: &PricingTable) -> bool {
    let rates = table.lookup(model);
    if model.contains(':') && rates.is_none() {
        return false;
    }
    rates.is_none_or(all_rates_are_zero)
}

/// Single pass over the events, so the token count, model list, estimate,
/// and mapping table cannot disagree with one another.
pub fn shadow_diagnostic(events: &[UsageEvent], table: &PricingTable) -> ShadowDiagnostic {
    let mut diagnostic = ShadowDiagnostic::default();
    let mut models = BTreeSet::new();
    for event in events.iter().filter(|e| is_unpriced(&e.model, table)) {
        let u = &event.usage;
        diagnostic.tokens += u.input + u.output + u.cache_write_5m + u.cache_write_1h + u.cache_read;
        models.insert(event.model.clone());
        // A model with no [shadow] entry contributes tokens and no dollars:
        // a rate is never invented for it.
        if let Some(reference) = table.shadow_reference(&event.model)
            && let Some(rates) = table.lookup(reference)
        {
            diagnostic.estimate += calculate(u, rates);
            diagnostic
                .mappings
                .insert(event.model.clone(), reference.to_owned());
        }
    }
    diagnostic.models = models.into_iter().collect();
    diagnostic
}
```

Add `use std::collections::BTreeMap;` to the module's imports if `BTreeSet` is
imported alone today.

- [ ] **Step 8: Run to verify the cost tests pass**

Run: `set -o pipefail; cargo test --lib cost:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 9: Wire into `DoctorReport`, `main.rs`, and both renderers**

In `src/scan.rs`, add to `DoctorReport` after `local_models`. Keep them as
plain scalars rather than storing `cost::ShadowDiagnostic` directly, so this
module stays pricing-agnostic exactly as its doc comment promises:

```rust
    /// Total tokens from models with no real rates (zero-rated or absent),
    /// excluding local `name:tag` models.
    pub unpriced_tokens: u64,
    /// How many distinct models those tokens came from.
    pub unpriced_model_count: usize,
    /// What those tokens would cost at `[shadow]` reference rates. Zero when
    /// nothing is mapped. Diagnostic only — no report total includes it.
    pub shadow_estimate: rust_decimal::Decimal,
    /// Observed unpriced model -> the reference model standing in for it.
    pub shadow_mappings: std::collections::BTreeMap<String, String>,
```

Widen `scan::doctor`'s parameter list to take these alongside the existing
`unpriced_models` / `zero_rated_models` / `local_models` arguments.

In `src/main.rs`, after the existing classification calls:

```rust
    let shadow = cost::shadow_diagnostic(&outcome.events, &pricing);
```

and pass `shadow.tokens`, `shadow.models.len()`, `shadow.estimate`, and
`shadow.mappings` through `render` into `scan::doctor`.

At the end of `resolve_pricing`, before returning the merged table — after
both override layers are merged, so a user table may reference a model
defined only in the embedded defaults:

```rust
    table.validate()?;
```

In `src/report/table.rs`, after the `Local models` row. Note `doctor` takes
no `precise` argument, so format money at the default two places:

```rust
    table.add_row(vec![
        "Unpriced tokens".to_owned(),
        if report.unpriced_tokens == 0 {
            "(none)".to_owned()
        } else {
            format!(
                "{} across {} models",
                group_thousands(report.unpriced_tokens),
                report.unpriced_model_count
            )
        },
    ]);
    table.add_row(vec![
        "Shadow estimate".to_owned(),
        if report.shadow_estimate.is_zero() {
            "(none)".to_owned()
        } else {
            format!(
                "{} at reference rates (see [shadow] in pricing)",
                money(report.shadow_estimate, false)
            )
        },
    ]);
```

Every existing construction of `DoctorReport` in tests must gain the four new
fields; `..Default::default()` will not work because the struct has no
`Default` impl, so add them explicitly.

In `src/report/json.rs`, add `unpriced_tokens: u64`, `shadow_estimate_usd: String`, and `shadow_mappings: BTreeMap<String, String>` to the doctor struct, populate them, and extend `doctor_contract`:

```rust
        assert_eq!(value["unpriced_tokens"], 0);
        assert_eq!(value["shadow_estimate_usd"], "0.00");
```

- [ ] **Step 10: Verify everything**

Run: `set -o pipefail; cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -20`
Expected: all green.

Then confirm against real data that no total moved:

```bash
cargo run --release -- monthly --since 2026-07-01 --until 2026-07-31
cargo run --release -- doctor | tail -12
```

Expected: the monthly cost is unchanged by Task 3, and `doctor` shows non-zero `Unpriced tokens` with a `Shadow estimate`.

- [ ] **Step 11: Document and commit**

Add a `[shadow]` note to the pricing section of `README.md` explaining that it feeds `doctor` only and never changes a reported cost.

```bash
git add src/pricing.rs src/cost.rs src/scan.rs src/main.rs src/report/table.rs src/report/json.rs pricing/default.toml README.md
git commit   # feat(doctor): report unpriced tokens and a [shadow] cost estimate
```

---

## Final verification

- [ ] `set -o pipefail; cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- [ ] `cargo run --release -- doctor` shows `Codex models backfilled` > 0 on the reference corpus
- [ ] `cargo run --release -- models --model codex-unknown` shows a materially smaller bucket than 434,311,580 tokens
- [ ] `cargo run --release -- cache` shows `Write 5m` / `Write 1h` and the 1-hour TTL sentence
- [ ] Update `tasks/todo.md` with a "Resuming From Here" section: done, next, blockers, assumptions
