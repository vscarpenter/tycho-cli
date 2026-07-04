//! Aggregation of deduplicated usage events into report buckets.
//!
//! Timestamps are stored in UTC; bucketing converts each event to the
//! requested timezone first, so "a day" means a calendar day where the
//! user lives (or UTC with `--utc`).

use chrono::NaiveDate;
use chrono_tz::Tz;

use crate::record::UsageEvent;

/// Token and cost totals for one bucket (a day, a month, a session, ...).
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
    /// Cost in USD (zero until the cost engine stamps events).
    pub cost: rust_decimal::Decimal,
}

impl Totals {
    /// Fold one event's usage and cost into this bucket.
    pub fn add(&mut self, event: &UsageEvent) {
        self.input += event.usage.input;
        self.output += event.usage.output;
        self.cache_write_5m += event.usage.cache_write_5m;
        self.cache_write_1h += event.usage.cache_write_1h;
        self.cache_read += event.usage.cache_read;
        self.cost += event.cost;
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
    for event in events.into_iter().filter(|e| in_range(e, tz, since, until)) {
        let date = event.timestamp.with_timezone(&tz).date_naive();
        buckets.entry(date).or_default().add(&event);
        total.add(&event);
    }
    DailyReport {
        days: buckets
            .into_iter()
            .map(|(date, totals)| DayTotals { date, totals })
            .collect(),
        total,
    }
}

/// True when the event's calendar date in `tz` falls inside the inclusive
/// `since..=until` range. Shared by every report.
pub(crate) fn in_range(
    event: &UsageEvent,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> bool {
    let date = event.timestamp.with_timezone(&tz).date_naive();
    !(since.is_some_and(|bound| date < bound) || until.is_some_and(|bound| date > bound))
}

/// Totals for one calendar month (`"2026-07"`) in the report timezone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthTotals {
    /// The month as `YYYY-MM` (sorts chronologically as a string).
    pub month: String,
    /// Token totals for that month.
    pub totals: Totals,
}

/// The `monthly` report: one row per month plus a grand total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonthlyReport {
    /// Months in ascending order.
    pub months: Vec<MonthTotals>,
    /// Grand totals across all included months.
    pub total: Totals,
}

/// Bucket events by calendar month in `tz`, within the inclusive
/// `since..=until` local-date range.
pub fn monthly(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> MonthlyReport {
    let mut buckets: std::collections::BTreeMap<String, Totals> = std::collections::BTreeMap::new();
    let mut total = Totals::default();
    for event in events.into_iter().filter(|e| in_range(e, tz, since, until)) {
        let month = event
            .timestamp
            .with_timezone(&tz)
            .format("%Y-%m")
            .to_string();
        buckets.entry(month).or_default().add(&event);
        total.add(&event);
    }
    MonthlyReport {
        months: buckets
            .into_iter()
            .map(|(month, totals)| MonthTotals { month, totals })
            .collect(),
        total,
    }
}

/// One session's rollup, subagent usage included via the shared session id.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTotals {
    /// Session UUID, or `(unknown)` when records lacked one.
    pub session_id: String,
    /// Encoded project directory the session ran in.
    pub project: String,
    /// Earliest record timestamp (UTC; render in the report timezone).
    pub start: chrono::DateTime<chrono::Utc>,
    /// Latest record timestamp.
    pub end: chrono::DateTime<chrono::Utc>,
    /// Distinct models used, sorted.
    pub models: Vec<String>,
    /// Token totals for the session.
    pub totals: Totals,
}

impl SessionTotals {
    /// Wall-clock span from first to last record, idle time included.
    pub fn duration(&self) -> chrono::Duration {
        self.end - self.start
    }
}

/// Sort order for the `sessions` report (always descending).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSort {
    /// Most recently started first (the default).
    Start,
    /// Largest total token count first.
    Tokens,
    /// Longest wall-clock span first.
    Duration,
}

/// The `sessions` report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionsReport {
    /// Sessions after sorting and `--limit`.
    pub sessions: Vec<SessionTotals>,
    /// How many sessions matched before the limit was applied.
    pub matching_sessions: usize,
    /// Grand totals across all matching sessions (not just the shown ones).
    pub total: Totals,
}

/// Group events into sessions, sort descending by `sort`, keep `limit`.
pub fn sessions(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    sort: SessionSort,
    limit: Option<usize>,
) -> SessionsReport {
    let mut buckets: std::collections::HashMap<String, SessionTotals> =
        std::collections::HashMap::new();
    let mut total = Totals::default();
    for event in events.into_iter().filter(|e| in_range(e, tz, since, until)) {
        total.add(&event);
        let id = event
            .session_id
            .clone()
            .unwrap_or_else(|| "(unknown)".to_owned());
        let session = buckets.entry(id.clone()).or_insert_with(|| SessionTotals {
            session_id: id,
            project: event.project.clone(),
            start: event.timestamp,
            end: event.timestamp,
            models: Vec::new(),
            totals: Totals::default(),
        });
        session.start = session.start.min(event.timestamp);
        session.end = session.end.max(event.timestamp);
        if !session.models.contains(&event.model) {
            session.models.push(event.model.clone());
        }
        session.totals.add(&event);
    }

    let mut sessions: Vec<SessionTotals> = buckets.into_values().collect();
    for session in &mut sessions {
        session.models.sort();
    }
    // Descending by the chosen key; session id breaks ties deterministically.
    sessions.sort_by(|a, b| {
        let key_order = match sort {
            SessionSort::Start => b.start.cmp(&a.start),
            SessionSort::Tokens => b.totals.total().cmp(&a.totals.total()),
            SessionSort::Duration => b.duration().cmp(&a.duration()),
        };
        key_order.then_with(|| a.session_id.cmp(&b.session_id))
    });
    let matching_sessions = sessions.len();
    if let Some(limit) = limit {
        sessions.truncate(limit);
    }
    SessionsReport {
        sessions,
        matching_sessions,
        total,
    }
}

/// One project's rollup.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTotals {
    /// Encoded project directory name.
    pub project: String,
    /// Distinct sessions observed in the window.
    pub sessions: u64,
    /// Latest record timestamp in the window.
    pub last_activity: chrono::DateTime<chrono::Utc>,
    /// Token totals for the project.
    pub totals: Totals,
}

/// The `projects` report, largest total first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectsReport {
    /// Projects, descending by total tokens.
    pub projects: Vec<ProjectTotals>,
    /// Grand totals.
    pub total: Totals,
}

/// Roll up events by project directory.
pub fn projects(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> ProjectsReport {
    struct Bucket {
        sessions: std::collections::BTreeSet<String>,
        last_activity: chrono::DateTime<chrono::Utc>,
        totals: Totals,
    }
    let mut buckets: std::collections::HashMap<String, Bucket> = std::collections::HashMap::new();
    let mut total = Totals::default();
    for event in events.into_iter().filter(|e| in_range(e, tz, since, until)) {
        total.add(&event);
        let bucket = buckets
            .entry(event.project.clone())
            .or_insert_with(|| Bucket {
                sessions: std::collections::BTreeSet::new(),
                last_activity: event.timestamp,
                totals: Totals::default(),
            });
        bucket.sessions.insert(
            event
                .session_id
                .clone()
                .unwrap_or_else(|| "(unknown)".to_owned()),
        );
        bucket.last_activity = bucket.last_activity.max(event.timestamp);
        bucket.totals.add(&event);
    }
    let mut projects: Vec<ProjectTotals> = buckets
        .into_iter()
        .map(|(project, bucket)| ProjectTotals {
            project,
            sessions: bucket.sessions.len() as u64,
            last_activity: bucket.last_activity,
            totals: bucket.totals,
        })
        .collect();
    projects.sort_by(|a, b| {
        b.totals
            .total()
            .cmp(&a.totals.total())
            .then_with(|| a.project.cmp(&b.project))
    });
    ProjectsReport { projects, total }
}

/// One model's rollup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTotals {
    /// Model id as recorded, e.g. `claude-opus-4-8`.
    pub model: String,
    /// Token totals for the model.
    pub totals: Totals,
}

/// The `models` report, largest total first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsReport {
    /// Models, descending by total tokens.
    pub models: Vec<ModelTotals>,
    /// Grand totals.
    pub total: Totals,
}

/// Roll up events by model id.
pub fn models(
    events: impl IntoIterator<Item = UsageEvent>,
    tz: Tz,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> ModelsReport {
    let mut buckets: std::collections::BTreeMap<String, Totals> = std::collections::BTreeMap::new();
    let mut total = Totals::default();
    for event in events.into_iter().filter(|e| in_range(e, tz, since, until)) {
        buckets.entry(event.model.clone()).or_default().add(&event);
        total.add(&event);
    }
    let mut models: Vec<ModelTotals> = buckets
        .into_iter()
        .map(|(model, totals)| ModelTotals { model, totals })
        .collect();
    models.sort_by(|a, b| {
        b.totals
            .total()
            .cmp(&a.totals.total())
            .then_with(|| a.model.cmp(&b.model))
    });
    ModelsReport { models, total }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{DedupKey, TokenUsage};
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
            cost: rust_decimal::Decimal::ZERO,
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

    #[test]
    fn stamped_costs_sum_into_buckets_and_grand_total() {
        let mut cheap = event_at("2026-07-01T10:00:00Z", 1);
        cheap.cost = "0.25".parse().unwrap();
        let mut pricey = event_at("2026-07-02T10:00:00Z", 1);
        pricey.cost = "1.50".parse().unwrap();

        let report = daily([cheap, pricey], chrono_tz::UTC, None, None);
        assert_eq!(report.days[0].totals.cost, "0.25".parse().unwrap());
        assert_eq!(report.total.cost, "1.75".parse().unwrap());
    }

    /// Full-control event builder for the grouping reports.
    fn rich_event(
        utc: &str,
        project: &str,
        session: Option<&str>,
        model: &str,
        output: u64,
    ) -> UsageEvent {
        let mut event = event_at(utc, output);
        event.project = project.to_owned();
        event.session_id = session.map(str::to_owned);
        event.model = model.to_owned();
        event
    }

    #[test]
    fn monthly_buckets_by_local_month_and_sorts_ascending() {
        // 03:00 UTC on July 1 is still June 30 in Chicago.
        let report = monthly(
            [
                event_at("2026-07-01T03:00:00Z", 7),
                event_at("2026-06-15T12:00:00Z", 5),
                event_at("2026-07-10T12:00:00Z", 11),
            ],
            chrono_tz::America::Chicago,
            None,
            None,
        );
        let months: Vec<_> = report.months.iter().map(|m| m.month.as_str()).collect();
        assert_eq!(months, ["2026-06", "2026-07"]);
        assert_eq!(report.months[0].totals.output, 12);
        assert_eq!(report.total.output, 23);
    }

    #[test]
    fn sessions_group_span_models_and_project() {
        let report = sessions(
            [
                rich_event("2026-07-01T10:00:00Z", "proj-a", Some("s1"), "opus", 5),
                rich_event("2026-07-01T12:30:00Z", "proj-a", Some("s1"), "sonnet", 7),
                rich_event("2026-07-01T11:00:00Z", "proj-a", Some("s1"), "opus", 9),
            ],
            chrono_tz::UTC,
            None,
            None,
            SessionSort::Start,
            None,
        );
        assert_eq!(report.sessions.len(), 1);
        let s = &report.sessions[0];
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.project, "proj-a");
        assert_eq!(s.models, ["opus", "sonnet"]);
        assert_eq!(s.duration(), chrono::Duration::minutes(150));
        assert_eq!(s.totals.output, 21);
    }

    #[test]
    fn sessions_without_an_id_land_in_the_unknown_bucket() {
        let report = sessions(
            [rich_event("2026-07-01T10:00:00Z", "p", None, "m", 1)],
            chrono_tz::UTC,
            None,
            None,
            SessionSort::Start,
            None,
        );
        assert_eq!(report.sessions[0].session_id, "(unknown)");
    }

    #[test]
    fn sessions_sort_by_tokens_and_limit_keeps_grand_total() {
        let report = sessions(
            [
                rich_event("2026-07-01T10:00:00Z", "p", Some("small"), "m", 1),
                rich_event("2026-07-02T10:00:00Z", "p", Some("big"), "m", 100),
                rich_event("2026-07-03T10:00:00Z", "p", Some("mid"), "m", 10),
            ],
            chrono_tz::UTC,
            None,
            None,
            SessionSort::Tokens,
            Some(2),
        );
        let ids: Vec<_> = report
            .sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(ids, ["big", "mid"]);
        assert_eq!(report.matching_sessions, 3);
        // The grand total covers all matching sessions, not just the shown ones.
        assert_eq!(report.total.output, 111);
    }

    #[test]
    fn sessions_sorted_by_most_recent_start_by_default() {
        let report = sessions(
            [
                rich_event("2026-07-01T10:00:00Z", "p", Some("old"), "m", 1),
                rich_event("2026-07-03T10:00:00Z", "p", Some("new"), "m", 1),
            ],
            chrono_tz::UTC,
            None,
            None,
            SessionSort::Start,
            None,
        );
        assert_eq!(report.sessions[0].session_id, "new");
    }

    #[test]
    fn projects_roll_up_with_session_counts_descending_by_total() {
        let report = projects(
            [
                rich_event("2026-07-01T10:00:00Z", "alpha", Some("s1"), "m", 1),
                rich_event("2026-07-02T10:00:00Z", "beta", Some("s2"), "m", 50),
                rich_event("2026-07-03T10:00:00Z", "beta", Some("s3"), "m", 50),
            ],
            chrono_tz::UTC,
            None,
            None,
        );
        assert_eq!(report.projects.len(), 2);
        assert_eq!(report.projects[0].project, "beta");
        assert_eq!(report.projects[0].sessions, 2);
        assert_eq!(
            report.projects[0].last_activity,
            "2026-07-03T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(report.total.output, 101);
    }

    #[test]
    fn models_roll_up_descending_by_total() {
        let report = models(
            [
                rich_event("2026-07-01T10:00:00Z", "p", Some("s"), "claude-sonnet-5", 1),
                rich_event(
                    "2026-07-01T11:00:00Z",
                    "p",
                    Some("s"),
                    "claude-opus-4-8",
                    50,
                ),
            ],
            chrono_tz::UTC,
            None,
            None,
        );
        assert_eq!(report.models[0].model, "claude-opus-4-8");
        assert_eq!(report.models[1].totals.output, 1);
    }

    #[test]
    fn monthly_and_sessions_respect_the_date_range() {
        let july_2 = NaiveDate::from_ymd_opt(2026, 7, 2).unwrap();
        let events = || {
            [
                rich_event("2026-07-01T10:00:00Z", "p", Some("s1"), "m", 1),
                rich_event("2026-07-02T10:00:00Z", "p", Some("s2"), "m", 2),
            ]
        };
        let m = monthly(events(), chrono_tz::UTC, Some(july_2), None);
        assert_eq!(m.total.output, 2);
        let s = sessions(
            events(),
            chrono_tz::UTC,
            Some(july_2),
            None,
            SessionSort::Start,
            None,
        );
        assert_eq!(s.matching_sessions, 1);
    }
}
