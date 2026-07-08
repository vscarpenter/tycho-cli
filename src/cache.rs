//! Cache economics — the flagship report (spec §4.3).
//!
//! For the selected window, per model and in total: the cache hit rate,
//! the actual cost, and the counterfactual cost the same tokens would have
//! incurred with zero caching (every cache write and read billed as plain
//! input). The difference is what prompt caching saved.

use chrono::NaiveDate;
use chrono_tz::Tz;
use rust_decimal::Decimal;

use crate::aggregate::Totals;
use crate::pricing::PricingTable;
use crate::record::UsageEvent;

/// Cache economics for one model (or the grand total).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheEconomics {
    /// Model id, or `Total` for the summary row.
    pub model: String,
    /// Token totals (with stamped actual cost).
    pub totals: Totals,
    /// `cache_read / (input + cache writes + cache_read)`; `None` when the
    /// denominator is zero.
    pub hit_rate: Option<Decimal>,
    /// What the window actually cost (stamped costs, so `--mode` applies).
    pub actual_cost: Decimal,
    /// What it would have cost with zero caching.
    pub counterfactual_cost: Decimal,
    /// `counterfactual - actual`.
    pub savings: Decimal,
    /// `counterfactual / actual`; `None` when actual is zero.
    pub leverage: Option<Decimal>,
}

/// The `cache` report: one row per model, largest actual cost first, plus
/// a grand total.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheReport {
    /// Per-model economics.
    pub models: Vec<CacheEconomics>,
    /// Economics across all models.
    pub total: CacheEconomics,
}

/// Build the cache-economics report. Events must already carry stamped
/// costs; the pricing table supplies input/output rates for the
/// counterfactual (unknown models contribute zero to both sides).
pub fn cache(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    table: &PricingTable,
) -> CacheReport {
    let by_model = crate::aggregate::models(events, tz, since, until);

    let mut models: Vec<CacheEconomics> = by_model
        .models
        .into_iter()
        .map(|entry| economics(entry.model, entry.totals, table))
        .collect();
    models.sort_by(|a, b| {
        b.actual_cost
            .cmp(&a.actual_cost)
            .then_with(|| a.model.cmp(&b.model))
    });

    // The grand-total counterfactual is the sum of per-model counterfactuals
    // (each model has its own input rate), not a re-price of summed tokens.
    let counterfactual = models.iter().map(|m| m.counterfactual_cost).sum();
    let total = with_counterfactual("Total".to_owned(), by_model.total, counterfactual);

    CacheReport { models, total }
}

/// Economics for one model's totals, pricing the counterfactual with that
/// model's own rates (zero for unpriced models).
fn economics(model: String, totals: Totals, table: &PricingTable) -> CacheEconomics {
    let counterfactual = table
        .lookup(&model)
        .map(|rates| {
            let million = Decimal::from(1_000_000u32);
            let all_input =
                totals.input + totals.cache_write_5m + totals.cache_write_1h + totals.cache_read;
            Decimal::from(all_input) * rates.input / million
                + Decimal::from(totals.output) * rates.output / million
        })
        .unwrap_or(Decimal::ZERO);
    with_counterfactual(model, totals, counterfactual)
}

fn with_counterfactual(model: String, totals: Totals, counterfactual: Decimal) -> CacheEconomics {
    let cacheable =
        totals.input + totals.cache_write_5m + totals.cache_write_1h + totals.cache_read;
    let hit_rate =
        (cacheable > 0).then(|| Decimal::from(totals.cache_read) / Decimal::from(cacheable));
    let actual = totals.cost;
    let leverage = (!actual.is_zero()).then(|| counterfactual / actual);
    CacheEconomics {
        model,
        totals,
        hit_rate,
        actual_cost: actual,
        counterfactual_cost: counterfactual,
        savings: counterfactual - actual,
        leverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Provider;
    use crate::record::{DedupKey, TokenUsage};

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn event(model: &str, usage: TokenUsage, cost: &str) -> UsageEvent {
        UsageEvent {
            timestamp: "2026-07-02T10:00:00Z".parse().unwrap(),
            session_id: None,
            project: String::new(),
            model: model.to_owned(),
            usage,
            cost_usd: None,
            cost: dec(cost),
            dedup_key: DedupKey::Uuid(format!("{model}-{}", usage.output)),
            provider: Provider::Claude,
        }
    }

    const USAGE: TokenUsage = TokenUsage {
        input: 100,
        output: 200,
        cache_write_5m: 50,
        cache_write_1h: 10,
        cache_read: 1_000,
    };

    #[test]
    fn computes_hit_rate_and_counterfactual_per_model() {
        let table = PricingTable::embedded();
        // opus 4-8: counterfactual = (100+50+10+1000)*5/1e6 + 200*25/1e6
        let report = cache(
            [event("claude-opus-4-8", USAGE, "0.0064125")],
            chrono_tz::UTC,
            None,
            None,
            &table,
        );
        let opus = &report.models[0];
        assert_eq!(opus.actual_cost, dec("0.0064125"));
        assert_eq!(opus.counterfactual_cost, dec("0.0108"));
        assert_eq!(opus.savings, dec("0.0043875"));
        // hit rate = 1000 / 1160
        let hit = opus.hit_rate.unwrap();
        assert!((hit - dec("0.8621")).abs() < dec("0.0001"), "{hit}");
        // leverage = 0.0108 / 0.0064125
        let leverage = opus.leverage.unwrap();
        assert!((leverage - dec("1.684")).abs() < dec("0.001"), "{leverage}");
    }

    #[test]
    fn guards_divide_by_zero() {
        let table = PricingTable::embedded();
        let zero_usage = TokenUsage {
            input: 0,
            output: 5,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 0,
        };
        let report = cache(
            [event("unknown-model", zero_usage, "0")],
            chrono_tz::UTC,
            None,
            None,
            &table,
        );
        assert_eq!(report.total.hit_rate, None); // no cacheable input at all
        assert_eq!(report.total.leverage, None); // actual cost is zero
        assert_eq!(report.total.counterfactual_cost, Decimal::ZERO); // unpriced
    }

    #[test]
    fn totals_sum_across_models_largest_actual_first() {
        let table = PricingTable::embedded();
        let report = cache(
            [
                event("claude-sonnet-5", USAGE, "0.01"),
                event("claude-opus-4-8", USAGE, "0.02"),
            ],
            chrono_tz::UTC,
            None,
            None,
            &table,
        );
        assert_eq!(report.models[0].model, "claude-opus-4-8");
        assert_eq!(report.total.actual_cost, dec("0.03"));
        // sonnet-5 counterfactual: 1160*2/1e6 + 200*10/1e6 = 0.00432
        assert_eq!(report.total.counterfactual_cost, dec("0.01512"));
        assert_eq!(report.total.model, "Total");
    }
}
