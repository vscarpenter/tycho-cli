//! Cost computation: modes, per-event math, unknown-model tracking.
//!
//! All currency math is [`Decimal`]. Costs are stamped onto events before
//! aggregation, so every report's buckets sum them for free.

use std::collections::BTreeSet;

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

/// Distinct model ids that have no pricing entry, sorted.
pub fn unknown_models(events: &[UsageEvent], table: &PricingTable) -> Vec<String> {
    events
        .iter()
        .filter(|event| table.lookup(&event.model).is_none())
        .map(|event| event.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    fn apply_stamps_costs_onto_events() {
        let table = PricingTable::embedded();
        let mut events = vec![event("claude-opus-4-8", None)];
        Coster::new(&table, CostMode::Calculate).apply(&mut events);
        assert_eq!(events[0].cost, dec(OPUS_EXPECTED));
    }
}
