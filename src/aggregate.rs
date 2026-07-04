//! Aggregation of deduplicated usage events into report buckets.
//!
//! Timestamps are stored in UTC; bucketing converts each event to the
//! requested timezone first, so "a day" means a calendar day where the
//! user lives (or UTC with `--utc`).

use chrono::NaiveDate;
use chrono_tz::Tz;

use crate::record::{TokenUsage, UsageEvent};

/// Token totals for one bucket (a day, and later a month, session, ...).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// Uncached input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Cache writes with a 5-minute TTL.
    pub cache_write_5m: u64,
    /// Cache writes with a 1-hour TTL.
    pub cache_write_1h: u64,
    /// Tokens read from cache.
    pub cache_read: u64,
}

impl Totals {
    /// Fold one event's usage into this bucket.
    pub fn add(&mut self, usage: &TokenUsage) {
        self.input += usage.input;
        self.output += usage.output;
        self.cache_write_5m += usage.cache_write_5m;
        self.cache_write_1h += usage.cache_write_1h;
        self.cache_read += usage.cache_read;
    }

    /// Sum of every token category.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write_5m + self.cache_write_1h + self.cache_read
    }
}

/// Totals for one calendar day in the report timezone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayTotals {
    /// The local calendar date.
    pub date: NaiveDate,
    /// Token totals for that date.
    pub totals: Totals,
}

/// The `daily` report: one row per day plus a grand total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DailyReport {
    /// Days in ascending date order.
    pub days: Vec<DayTotals>,
    /// Grand totals across all included days.
    pub total: Totals,
}

/// Bucket events by calendar date in `tz`, keeping only dates within the
/// inclusive `since..=until` range (each bound optional).
pub fn daily(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> DailyReport {
    // BTreeMap keeps buckets in date order, so the report needs no sort.
    let mut buckets: std::collections::BTreeMap<NaiveDate, Totals> =
        std::collections::BTreeMap::new();
    let mut total = Totals::default();
    for event in events {
        let date = event.timestamp.with_timezone(&tz).date_naive();
        if since.is_some_and(|bound| date < bound) || until.is_some_and(|bound| date > bound) {
            continue;
        }
        buckets.entry(date).or_default().add(&event.usage);
        total.add(&event.usage);
    }
    DailyReport {
        days: buckets
            .into_iter()
            .map(|(date, totals)| DayTotals { date, totals })
            .collect(),
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::DedupKey;
    use chrono::{DateTime, Utc};

    fn event_at(utc: &str, output: u64) -> UsageEvent {
        UsageEvent {
            timestamp: utc.parse::<DateTime<Utc>>().unwrap(),
            session_id: None,
            project: String::new(),
            model: "m".into(),
            usage: TokenUsage {
                input: 1,
                output,
                cache_write_5m: 10,
                cache_write_1h: 20,
                cache_read: 100,
            },
            cost_usd: None,
            dedup_key: DedupKey::Uuid(format!("{utc}-{output}")),
        }
    }

    #[test]
    fn buckets_by_local_date_in_the_requested_timezone() {
        // 04:30 UTC on July 3 is still July 2 in Chicago (UTC-5 in summer).
        let report = daily(
            [event_at("2026-07-03T04:30:00Z", 5)],
            chrono_tz::America::Chicago,
            None,
            None,
        );
        assert_eq!(report.days.len(), 1);
        assert_eq!(
            report.days[0].date,
            NaiveDate::from_ymd_opt(2026, 7, 2).unwrap()
        );
    }

    #[test]
    fn the_same_instant_lands_on_a_different_day_in_utc() {
        let report = daily(
            [event_at("2026-07-03T04:30:00Z", 5)],
            chrono_tz::UTC,
            None,
            None,
        );
        assert_eq!(
            report.days[0].date,
            NaiveDate::from_ymd_opt(2026, 7, 3).unwrap()
        );
    }

    #[test]
    fn sums_every_token_category_per_day_and_grand_total() {
        let report = daily(
            [
                event_at("2026-07-01T10:00:00Z", 5),
                event_at("2026-07-01T11:00:00Z", 7),
                event_at("2026-07-02T10:00:00Z", 11),
            ],
            chrono_tz::UTC,
            None,
            None,
        );
        assert_eq!(report.days.len(), 2);
        let day1 = &report.days[0];
        assert_eq!(day1.totals.input, 2);
        assert_eq!(day1.totals.output, 12);
        assert_eq!(day1.totals.cache_write_5m, 20);
        assert_eq!(day1.totals.cache_write_1h, 40);
        assert_eq!(day1.totals.cache_read, 200);
        assert_eq!(report.total.output, 23);
        assert_eq!(report.total.total(), 3 + 23 + 30 + 60 + 300);
    }

    #[test]
    fn days_come_out_sorted_ascending_regardless_of_input_order() {
        let report = daily(
            [
                event_at("2026-07-03T10:00:00Z", 1),
                event_at("2026-07-01T10:00:00Z", 1),
                event_at("2026-07-02T10:00:00Z", 1),
            ],
            chrono_tz::UTC,
            None,
            None,
        );
        let dates: Vec<_> = report.days.iter().map(|d| d.date.to_string()).collect();
        assert_eq!(dates, ["2026-07-01", "2026-07-02", "2026-07-03"]);
    }

    #[test]
    fn since_and_until_are_inclusive_local_date_bounds() {
        let bound = NaiveDate::from_ymd_opt(2026, 7, 2).unwrap();
        let report = daily(
            [
                event_at("2026-07-01T10:00:00Z", 1),
                event_at("2026-07-02T10:00:00Z", 2),
                event_at("2026-07-03T10:00:00Z", 4),
            ],
            chrono_tz::UTC,
            Some(bound),
            Some(bound),
        );
        assert_eq!(report.days.len(), 1);
        assert_eq!(report.days[0].date, bound);
        assert_eq!(report.total.output, 2);
    }

    #[test]
    fn empty_input_yields_an_empty_report() {
        let report = daily([], chrono_tz::UTC, None, None);
        assert!(report.days.is_empty());
        assert_eq!(report.total, Totals::default());
    }
}
