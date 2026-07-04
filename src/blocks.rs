//! Billing blocks — activity-anchored 5-hour windows (Phase 5A).
//!
//! Claude's usage limits reset on rolling 5-hour windows. This report groups
//! deduplicated events into blocks anchored at the first message (floored to
//! the hour); a new block starts whenever an event lands at or after the
//! current window's end. Because the anchor is hour-floored, a >=5h inactivity
//! gap always crosses a window end, so this single check subsumes ccusage's
//! explicit gap rule. Unlike the group-by reports, block membership depends on
//! running state, so this is a fold, not a `BTreeMap` bucketing.

use chrono::{DateTime, Duration, DurationRound, NaiveDate, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;

use crate::aggregate::{self, Totals};
use crate::record::UsageEvent;

/// Length of a billing block, in hours.
pub const BLOCK_HOURS: i64 = 5;

/// Where the active block lands if its current rate holds to the window end.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockProjection {
    /// Cost extrapolated to the full 5-hour window.
    pub projected_cost: Decimal,
    /// Total tokens extrapolated to the full window.
    pub projected_tokens: u64,
    /// Tokens per minute over the block so far.
    pub burn_tokens_per_min: f64,
    /// Time left until the window closes.
    pub remaining: Duration,
}

/// One 5-hour billing block.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockTotals {
    /// The hour-floored anchor.
    pub start: DateTime<Utc>,
    /// `start + BLOCK_HOURS` (nominal window end).
    pub end: DateTime<Utc>,
    /// First event in the block.
    pub first_activity: DateTime<Utc>,
    /// Last event in the block.
    pub last_activity: DateTime<Utc>,
    /// Token and cost totals for the block.
    pub totals: Totals,
    /// Distinct models used, sorted.
    pub models: Vec<String>,
    /// Projection, present only on the active block.
    pub projection: Option<BlockProjection>,
}

/// The `blocks` report: activity blocks oldest-first, plus a grand total.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlocksReport {
    /// Blocks in ascending start order.
    pub blocks: Vec<BlockTotals>,
    /// Grand totals across all blocks.
    pub total: Totals,
}

/// Group events into activity-anchored 5-hour blocks. `now` marks the active
/// block (the window containing it) and drives its projection.
pub fn blocks(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> BlocksReport {
    let mut events: Vec<UsageEvent> = events
        .into_iter()
        .filter(|e| aggregate::in_range(e, tz, since, until))
        .collect();
    events.sort_by_key(|e| e.timestamp);

    let window = Duration::hours(BLOCK_HOURS);
    let mut blocks: Vec<BlockTotals> = Vec::new();
    let mut total = Totals::default();

    for event in &events {
        total.add(event);
        let start_new = blocks.last().is_none_or(|b| event.timestamp >= b.end);
        if start_new {
            let start = floor_hour(event.timestamp);
            blocks.push(BlockTotals {
                start,
                end: start + window,
                first_activity: event.timestamp,
                last_activity: event.timestamp,
                totals: Totals::default(),
                models: Vec::new(),
                projection: None,
            });
        }
        if let Some(block) = blocks.last_mut() {
            block.totals.add(event);
            block.last_activity = block.last_activity.max(event.timestamp);
            if !block.models.contains(&event.model) {
                block.models.push(event.model.clone());
            }
        }
    }

    for block in &mut blocks {
        block.models.sort();
        if block.start <= now && now < block.end {
            block.projection = Some(project(block, now));
        }
    }
    BlocksReport { blocks, total }
}

/// Truncate a timestamp down to the hour (truncation is relative to the Unix
/// epoch, which sits on an hour boundary).
fn floor_hour(ts: DateTime<Utc>) -> DateTime<Utc> {
    ts.duration_trunc(Duration::hours(1)).unwrap_or(ts)
}

/// Linear run-rate projection: scale the block's cost/tokens by
/// `window / elapsed`. Cost stays in `Decimal`; tokens and burn are floats.
fn project(block: &BlockTotals, now: DateTime<Utc>) -> BlockProjection {
    let window_secs = BLOCK_HOURS * 3_600;
    let elapsed_secs = (now - block.start).num_seconds().clamp(1, window_secs);
    let scale_dec = Decimal::from(window_secs) / Decimal::from(elapsed_secs);
    let scale_f = window_secs as f64 / elapsed_secs as f64;
    let actual_tokens = block.totals.total();
    BlockProjection {
        projected_cost: block.totals.cost * scale_dec,
        projected_tokens: (actual_tokens as f64 * scale_f).round() as u64,
        burn_tokens_per_min: actual_tokens as f64 / (elapsed_secs as f64 / 60.0),
        remaining: block.end - now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DedupKey, TokenUsage};

    fn ev(utc: &str, model: &str, tokens: u64, cost: &str) -> UsageEvent {
        UsageEvent {
            timestamp: utc.parse().unwrap(),
            session_id: Some("s".into()),
            project: "p".into(),
            model: model.into(),
            usage: TokenUsage {
                input: tokens,
                output: 0,
                cache_write_5m: 0,
                cache_write_1h: 0,
                cache_read: 0,
            },
            cost_usd: None,
            cost: cost.parse().unwrap(),
            dedup_key: DedupKey::Uuid(format!("{utc}-{model}")),
        }
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn events_within_five_hours_form_one_block() {
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-04T13:50:00Z", "opus", 200, "2.00"),
            ],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T20:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 1);
        let b = &report.blocks[0];
        assert_eq!(b.start, utc("2026-07-04T09:00:00Z"));
        assert_eq!(b.end, utc("2026-07-04T14:00:00Z"));
        assert_eq!(b.totals.total(), 300);
        assert_eq!(b.totals.cost, dec("3.00"));
        assert!(b.projection.is_none()); // now is past the block end
    }

    #[test]
    fn an_event_past_the_window_starts_a_new_block() {
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-04T14:30:00Z", "opus", 50, "0.50"), // >= 14:00 -> new block
            ],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T20:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 2);
        assert_eq!(report.blocks[1].start, utc("2026-07-04T14:00:00Z"));
        assert_eq!(report.total.total(), 150);
    }

    #[test]
    fn a_long_gap_starts_a_new_block() {
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-05T09:14:00Z", "opus", 100, "1.00"),
            ],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-06T00:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 2);
    }

    #[test]
    fn the_active_block_gets_a_projection() {
        // now is 2.5h into a 5h block anchored at 12:00 -> x2 scale
        let report = blocks(
            vec![ev("2026-07-04T12:00:00Z", "opus", 100, "2.00")],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T14:30:00Z"),
        );
        let b = &report.blocks[0];
        let proj = b.projection.as_ref().unwrap();
        assert_eq!(proj.projected_cost, dec("4.00"));
        assert_eq!(proj.projected_tokens, 200);
        assert_eq!(proj.remaining, Duration::minutes(150)); // 17:00 - 14:30
    }

    #[test]
    fn empty_input_yields_an_empty_report() {
        let report = blocks(
            vec![],
            chrono_tz::UTC,
            None,
            None,
            utc("2026-07-04T12:00:00Z"),
        );
        assert!(report.blocks.is_empty());
        assert_eq!(report.total, Totals::default());
    }

    #[test]
    fn since_until_narrows_the_events() {
        let jul5 = NaiveDate::from_ymd_opt(2026, 7, 5).unwrap();
        let report = blocks(
            vec![
                ev("2026-07-04T09:14:00Z", "opus", 100, "1.00"),
                ev("2026-07-05T09:14:00Z", "opus", 200, "2.00"),
            ],
            chrono_tz::UTC,
            Some(jul5),
            None,
            utc("2026-07-06T00:00:00Z"),
        );
        assert_eq!(report.blocks.len(), 1);
        assert_eq!(report.total.total(), 200);
    }
}
