//! Machine-readable JSON output — the scripting API.
//!
//! The shape below is a documented contract (`docs/json-schema.md`):
//! adding fields is allowed, renaming or removing them is a breaking
//! change. Token counts keep the 5m/1h cache-write split that the human
//! table combines. The nested `tokens` object leaves room for a sibling
//! `cost` object in Phase 3 without breaking consumers.

use serde::Serialize;

use crate::aggregate::{DailyReport, Totals};

/// One serialized token block.
#[derive(Serialize)]
struct TokensOut {
    input: u64,
    output: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
    total: u64,
}

impl From<&Totals> for TokensOut {
    fn from(totals: &Totals) -> Self {
        Self {
            input: totals.input,
            output: totals.output,
            cache_write_5m: totals.cache_write_5m,
            cache_write_1h: totals.cache_write_1h,
            cache_read: totals.cache_read,
            total: totals.total(),
        }
    }
}

#[derive(Serialize)]
struct DayOut {
    date: String,
    tokens: TokensOut,
}

#[derive(Serialize)]
struct DailyOut {
    command: &'static str,
    timezone: String,
    days: Vec<DayOut>,
    totals: TokensOut,
}

/// Render the daily report as pretty-printed JSON.
pub fn daily(report: &DailyReport, timezone: &str) -> String {
    let out = DailyOut {
        command: "daily",
        timezone: timezone.to_owned(),
        days: report
            .days
            .iter()
            .map(|day| DayOut {
                date: day.date.to_string(),
                tokens: TokensOut::from(&day.totals),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    // A struct of plain fields cannot fail to serialize; fall back to "{}"
    // rather than panicking if serde_json ever disagrees.
    serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::DayTotals;
    use chrono::NaiveDate;

    fn sample_report() -> DailyReport {
        DailyReport {
            days: vec![DayTotals {
                date: NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                totals: Totals {
                    input: 2,
                    output: 5_301,
                    cache_write_5m: 4_000,
                    cache_write_1h: 1_521,
                    cache_read: 196_377,
                },
            }],
            total: Totals {
                input: 2,
                output: 5_301,
                cache_write_5m: 4_000,
                cache_write_1h: 1_521,
                cache_read: 196_377,
            },
        }
    }

    #[test]
    fn matches_the_documented_contract() {
        let value: serde_json::Value =
            serde_json::from_str(&daily(&sample_report(), "America/Chicago")).unwrap();
        assert_eq!(value["command"], "daily");
        assert_eq!(value["timezone"], "America/Chicago");
        assert_eq!(value["days"][0]["date"], "2026-07-02");
        assert_eq!(value["days"][0]["tokens"]["input"], 2);
        assert_eq!(value["days"][0]["tokens"]["cache_write_5m"], 4000);
        assert_eq!(value["days"][0]["tokens"]["cache_write_1h"], 1521);
        assert_eq!(
            value["days"][0]["tokens"]["total"],
            2 + 5301 + 4000 + 1521 + 196377
        );
        assert_eq!(value["totals"]["cache_read"], 196377);
    }

    #[test]
    fn empty_report_is_valid_json_with_empty_days() {
        let value: serde_json::Value =
            serde_json::from_str(&daily(&DailyReport::default(), "UTC")).unwrap();
        assert_eq!(value["days"].as_array().unwrap().len(), 0);
        assert_eq!(value["totals"]["total"], 0);
    }
}
