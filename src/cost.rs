//! Cost computation: modes, per-event math, unknown-model tracking.
//!
//! All currency math is [`Decimal`]. Costs are stamped onto events before
//! aggregation, so every report's buckets sum them for free.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;

use crate::pricing::{ModelPricing, PricingTable};
use crate::record::{TokenUsage, UsageEvent};

/// How costs are derived, mirroring ccusage semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CostMode {
    /// Use a record's `costUSD` when present, else calculate (the default).
    #[default]
    Auto,
    /// Always calculate from tokens and the pricing table.
    Calculate,
    /// Only sum recorded `costUSD` values (legacy transcripts).
    Display,
}

/// Computes per-event cost against a pricing table.
#[derive(Debug, Clone, Copy)]
pub struct Coster<'a> {
    table: &'a PricingTable,
    mode: CostMode,
}

impl<'a> Coster<'a> {
    /// Build a coster for one scan.
    pub fn new(table: &'a PricingTable, mode: CostMode) -> Self {
        Self { table, mode }
    }

    /// Cost of one event in USD. Unknown models cost zero (spec §4.1);
    /// they are surfaced via [`unknown_models`] and `doctor`, never a crash.
    pub fn cost(&self, event: &UsageEvent) -> Decimal {
        // NaN or infinite recorded costs degrade to zero rather than panic.
        let recorded = |value: Option<f64>| {
            value
                .and_then(Decimal::from_f64_retain)
                .unwrap_or(Decimal::ZERO)
        };
        let calculated = || {
            self.table
                .lookup(&event.model)
                .map(|rates| calculate(&event.usage, rates))
                .unwrap_or(Decimal::ZERO)
        };
        match self.mode {
            CostMode::Calculate => calculated(),
            CostMode::Display => recorded(event.cost_usd),
            CostMode::Auto => match event.cost_usd {
                Some(value) => recorded(Some(value)),
                None => calculated(),
            },
        }
    }

    /// Stamp costs onto a batch of events.
    pub fn apply(&self, events: &mut [UsageEvent]) {
        for event in events {
            event.cost = self.cost(event);
        }
    }
}

/// Distinct model ids that have no pricing entry, sorted. Ollama-style
/// `name:tag` ids are excluded — those are reported separately by
/// [`local_models`] and never warned about.
pub fn unknown_models(events: &[UsageEvent], table: &PricingTable) -> Vec<String> {
    events
        .iter()
        .filter(|event| !event.model.contains(':') && table.lookup(&event.model).is_none())
        .map(|event| event.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Distinct observed model ids whose matched pricing entry has every rate
/// at zero: explicitly zero-rated (e.g. a Codex label with no public API
/// price), as opposed to genuinely unpriced.
pub fn zero_rated_models(events: &[UsageEvent], table: &PricingTable) -> Vec<String> {
    events
        .iter()
        .filter(|event| table.lookup(&event.model).is_some_and(all_rates_are_zero))
        .map(|event| event.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Distinct observed model ids with no pricing entry whose id contains a
/// `:` (an Ollama-style `name:tag` id) — local models, not warned about.
pub fn local_models(events: &[UsageEvent], table: &PricingTable) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.model.contains(':') && table.lookup(&event.model).is_none())
        .map(|event| event.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// What `doctor` reports about tokens carrying no real rates: how many there
/// are, which models they came from, what they would cost at `[shadow]`
/// reference rates, and which reference each model used.
///
/// Diagnostic only. Nothing here reaches [`Coster`], and no report total
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

/// Build the shadow diagnostic in a single pass, so the token count, model
/// list, estimate, and mapping table cannot disagree with one another.
pub fn shadow_diagnostic(events: &[UsageEvent], table: &PricingTable) -> ShadowDiagnostic {
    let mut diagnostic = ShadowDiagnostic::default();
    let mut models = BTreeSet::new();
    for event in events.iter().filter(|e| is_unpriced(&e.model, table)) {
        let usage = &event.usage;
        diagnostic.tokens += usage.input
            + usage.output
            + usage.cache_write_5m
            + usage.cache_write_1h
            + usage.cache_read;
        models.insert(event.model.clone());
        // A model with no [shadow] entry contributes tokens and no dollars:
        // a rate is never invented for it.
        if let Some(reference) = table.shadow_reference(&event.model)
            && let Some(rates) = table.lookup(reference)
        {
            diagnostic.estimate += calculate(usage, rates);
            diagnostic
                .mappings
                .insert(event.model.clone(), reference.to_owned());
        }
    }
    diagnostic.models = models.into_iter().collect();
    diagnostic
}

fn all_rates_are_zero(rates: &ModelPricing) -> bool {
    rates.input == Decimal::ZERO
        && rates.output == Decimal::ZERO
        && rates.cache_write_5m == Decimal::ZERO
        && rates.cache_write_1h == Decimal::ZERO
        && rates.cache_read == Decimal::ZERO
}

/// `tokens x rate-per-million`, exact in decimal.
fn calculate(usage: &TokenUsage, rates: &ModelPricing) -> Decimal {
    let per_million =
        |tokens: u64, rate: Decimal| Decimal::from(tokens) * rate / Decimal::from(1_000_000u32);
    per_million(usage.input, rates.input)
        + per_million(usage.output, rates.output)
        + per_million(usage.cache_write_5m, rates.cache_write_5m)
        + per_million(usage.cache_write_1h, rates.cache_write_1h)
        + per_million(usage.cache_read, rates.cache_read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Provider;
    use crate::record::DedupKey;

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn event(model: &str, cost_usd: Option<f64>) -> UsageEvent {
        UsageEvent {
            timestamp: "2026-07-02T10:00:00Z".parse().unwrap(),
            session_id: None,
            project: String::new(),
            model: model.to_owned(),
            usage: TokenUsage {
                input: 100,
                output: 200,
                cache_write_5m: 50,
                cache_write_1h: 10,
                cache_read: 1_000,
            },
            cost_usd,
            cost: Decimal::ZERO,
            dedup_key: DedupKey::Uuid("u".into()),
            provider: Provider::Claude,
        }
    }

    // Every `event` carries 100+200+50+10+1000 = 1360 tokens.
    const EVENT_TOKENS: u64 = 1_360;

    /// Local `name:tag` models cost nothing as a fact, not as a gap, so they
    /// are excluded. Priced models are excluded too.
    #[test]
    fn unpriced_tally_counts_only_models_with_no_real_rates() {
        let table = PricingTable::embedded();
        let events = [
            event("qwen3.6:27b", None),   // local: excluded
            event("gpt-5-codex", None),   // zero-rated: counted
            event("claude-opus-5", None), // priced: excluded
        ];
        let diagnostic = shadow_diagnostic(&events, &table);
        assert_eq!(diagnostic.tokens, EVENT_TOKENS);
        assert_eq!(diagnostic.models, ["gpt-5-codex"]);
    }

    /// A zero-rated model with a `[shadow]` entry contributes dollars; one
    /// without contributes only tokens. gpt-5.4 stands in for gpt-5-codex:
    /// (100*2.5 + 200*15 + 50*2.5 + 10*2.5 + 1000*0.25) / 1e6.
    #[test]
    fn shadow_estimate_prices_only_mapped_models() {
        let table = PricingTable::embedded();
        let mapped = shadow_diagnostic(&[event("gpt-5-codex", None)], &table);
        assert_eq!(mapped.estimate, dec("0.00365"));
        assert_eq!(mapped.mappings["gpt-5-codex"], "gpt-5.4");

        // codex-unknown is deliberately unmapped: no rate is invented for a
        // model whose defining property is that it is unknown.
        let unmapped = shadow_diagnostic(&[event("codex-unknown", None)], &table);
        assert_eq!(unmapped.estimate, Decimal::ZERO);
        assert_eq!(unmapped.tokens, EVENT_TOKENS);
        assert!(unmapped.mappings.is_empty());
    }

    /// The diagnostic is read-only: it must not disturb the stamped costs
    /// any report actually sums.
    #[test]
    fn shadow_diagnostic_does_not_change_stamped_costs() {
        let table = PricingTable::embedded();
        let mut events = [event("gpt-5-codex", None), event("claude-opus-4-8", None)];
        Coster::new(&table, CostMode::Calculate).apply(&mut events);
        let before: Vec<Decimal> = events.iter().map(|e| e.cost).collect();
        let _ = shadow_diagnostic(&events, &table);
        let after: Vec<Decimal> = events.iter().map(|e| e.cost).collect();
        assert_eq!(before, after);
        assert_eq!(before[0], Decimal::ZERO); // still zero-rated
    }

    // opus 4-8: (100*5 + 200*25 + 50*6.25 + 10*10 + 1000*0.5) / 1e6
    const OPUS_EXPECTED: &str = "0.0064125";

    #[test]
    fn calculate_mode_prices_from_tokens() {
        let table = PricingTable::embedded();
        let coster = Coster::new(&table, CostMode::Calculate);
        assert_eq!(
            coster.cost(&event("claude-opus-4-8", None)),
            dec(OPUS_EXPECTED)
        );
        // recorded costUSD is ignored in calculate mode
        assert_eq!(
            coster.cost(&event("claude-opus-4-8", Some(0.5))),
            dec(OPUS_EXPECTED)
        );
    }

    #[test]
    fn auto_mode_prefers_recorded_cost_usd() {
        let table = PricingTable::embedded();
        let coster = Coster::new(&table, CostMode::Auto);
        assert_eq!(
            coster.cost(&event("claude-opus-4-8", Some(0.5))),
            dec("0.5")
        );
        assert_eq!(
            coster.cost(&event("claude-opus-4-8", None)),
            dec(OPUS_EXPECTED)
        );
    }

    #[test]
    fn display_mode_only_sums_recorded_costs() {
        let table = PricingTable::embedded();
        let coster = Coster::new(&table, CostMode::Display);
        assert_eq!(
            coster.cost(&event("claude-opus-4-8", Some(0.5))),
            dec("0.5")
        );
        assert_eq!(coster.cost(&event("claude-opus-4-8", None)), Decimal::ZERO);
    }

    #[test]
    fn unknown_models_cost_zero_and_are_reported() {
        let table = PricingTable::embedded();
        let coster = Coster::new(&table, CostMode::Calculate);
        let events = vec![
            event("mystery-model-9", None),
            event("claude-opus-4-8", None),
            event("mystery-model-9", None),
        ];
        assert_eq!(coster.cost(&events[0]), Decimal::ZERO);
        assert_eq!(unknown_models(&events, &table), ["mystery-model-9"]);
    }

    #[test]
    fn local_and_zero_rated_models_are_split_out() {
        let table = PricingTable::embedded();
        let events = vec![
            event("qwen3.6:27b", None),     // local: ':' id, no entry, no warning
            event("gpt-5.2-codex", None),   // zero-rated entry
            event("brand-new-model", None), // unknown: warns
        ];
        assert_eq!(local_models(&events, &table), vec!["qwen3.6:27b"]);
        assert_eq!(zero_rated_models(&events, &table), vec!["gpt-5.2-codex"]);
        assert_eq!(unknown_models(&events, &table), vec!["brand-new-model"]);
    }

    #[test]
    fn apply_stamps_costs_onto_events() {
        let table = PricingTable::embedded();
        let mut events = vec![event("claude-opus-4-8", None)];
        Coster::new(&table, CostMode::Calculate).apply(&mut events);
        assert_eq!(events[0].cost, dec(OPUS_EXPECTED));
    }
}
