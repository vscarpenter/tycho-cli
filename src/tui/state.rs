//! The pure dashboard model: [`DashboardState`] and its derivation.
//!
//! Everything time-relative takes an injected `now`, never `Utc::now()`, so
//! the burn rate and active-session logic are deterministic under test.

use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;

use crate::cache::{self, CacheEconomics};
use crate::pricing::PricingTable;
use crate::record::UsageEvent;

/// Width of the burn-rate window, in minutes (also the sparkline length).
pub const BURN_WINDOW_MINUTES: i64 = 10;

/// A transcript file touched within this many seconds of `now` marks its
/// project "active" (spec §5.2, "recent file mtime").
pub const ACTIVE_THRESHOLD_SECS: i64 = 300;

/// Tokens-per-minute burn over the last [`BURN_WINDOW_MINUTES`] minutes.
#[derive(Debug, Clone, PartialEq)]
pub struct BurnRate {
    /// Total window tokens divided by the window width in minutes.
    pub tokens_per_min: f64,
    /// Window cost projected to an hourly run-rate.
    pub cost_per_hour: Decimal,
    /// Tokens per minute, oldest (index 0) to newest (index 9).
    pub per_minute: [u64; 10],
}

/// One row of the per-model split for today.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelLine {
    /// Model id.
    pub model: String,
    /// Total tokens across every category.
    pub tokens: u64,
    /// Actual cost (honors `--mode`).
    pub cost: Decimal,
    /// Cache hit rate, `None` when there was no cacheable input.
    pub hit_rate: Option<Decimal>,
}

impl From<&CacheEconomics> for ModelLine {
    fn from(row: &CacheEconomics) -> Self {
        Self {
            model: row.model.clone(),
            tokens: row.totals.total(),
            cost: row.actual_cost,
            hit_rate: row.hit_rate,
        }
    }
}

/// One row of today's sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLine {
    /// Session id.
    pub session_id: String,
    /// Encoded project directory.
    pub project: String,
    /// Total tokens.
    pub tokens: u64,
    /// Actual cost.
    pub cost: Decimal,
    /// Latest event timestamp.
    pub last_activity: DateTime<Utc>,
    /// Whether this is the live session.
    pub active: bool,
}

/// The active session detected via file mtime.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSession {
    /// Encoded project directory of the most-recently-touched transcript.
    pub project: String,
    /// How long since that file was last written.
    pub idle: Duration,
}

/// A live snapshot of usage, everything the dashboard renders.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardState {
    /// Today's totals plus cache economics (drives the headline + gauge).
    pub today: CacheEconomics,
    /// Burn rate over the last [`BURN_WINDOW_MINUTES`] minutes.
    pub burn: BurnRate,
    /// Per-model split for today, largest actual cost first.
    pub models: Vec<ModelLine>,
    /// Today's sessions, newest activity first.
    pub sessions: Vec<SessionLine>,
    /// The active session detected via file mtime, if any.
    pub active: Option<ActiveSession>,
    /// When this snapshot was computed (the injected `now`).
    pub generated_at: DateTime<Utc>,
}

impl DashboardState {
    /// Derive a snapshot from cost-stamped `events`, file `mtimes`, and the
    /// current instant `now`. Today's aggregations use `now`'s local date in
    /// `tz`; the burn rate uses the absolute last-10-minute window.
    pub fn derive(
        events: Vec<UsageEvent>,
        mtimes: &[(String, SystemTime)],
        now: DateTime<Utc>,
        tz: Tz,
        pricing: &PricingTable,
    ) -> Self {
        let today_date = now.with_timezone(&tz).date_naive();
        let today_events: Vec<UsageEvent> = events
            .iter()
            .filter(|e| e.timestamp.with_timezone(&tz).date_naive() == today_date)
            .cloned()
            .collect();

        let cache_report = cache::cache(today_events.clone(), tz, None, None, pricing);
        let models = cache_report.models.iter().map(ModelLine::from).collect();

        let sessions = session_lines(today_events, tz);
        let active = active_project(mtimes, now, ACTIVE_THRESHOLD_SECS);
        let burn = burn_rate(&events, now);

        Self {
            today: cache_report.total,
            burn,
            models,
            sessions: mark_active(sessions, active.as_ref()),
            active,
            generated_at: now,
        }
    }
}

/// Sum every token category of one event.
fn event_tokens(event: &UsageEvent) -> u64 {
    let u = &event.usage;
    u.input + u.output + u.cache_write_5m + u.cache_write_1h + u.cache_read
}

/// Compute the burn rate over the last [`BURN_WINDOW_MINUTES`] minutes ending
/// at `now`. Events outside the window contribute nothing.
pub fn burn_rate(events: &[UsageEvent], now: DateTime<Utc>) -> BurnRate {
    let window_start = now - Duration::minutes(BURN_WINDOW_MINUTES);
    let mut per_minute = [0u64; 10];
    let mut window_tokens = 0u64;
    let mut window_cost = Decimal::ZERO;
    for event in events
        .iter()
        .filter(|e| e.timestamp > window_start && e.timestamp <= now)
    {
        let tokens = event_tokens(event);
        window_tokens += tokens;
        window_cost += event.cost;
        // 0 minutes ago is the newest bucket (index 9); 9 is the oldest.
        let mins_ago = now.signed_duration_since(event.timestamp).num_minutes();
        let idx = (9 - mins_ago).clamp(0, 9) as usize;
        per_minute[idx] += tokens;
    }
    BurnRate {
        tokens_per_min: window_tokens as f64 / BURN_WINDOW_MINUTES as f64,
        cost_per_hour: window_cost * Decimal::from(60 / BURN_WINDOW_MINUTES),
        per_minute,
    }
}

/// Roll today's events into session rows (fully implemented in the next
/// task); a stub for now so the crate compiles.
fn session_lines(_events: Vec<UsageEvent>, _tz: Tz) -> Vec<SessionLine> {
    Vec::new()
}

/// Flag the live session (fully implemented in the next task).
fn mark_active(sessions: Vec<SessionLine>, _active: Option<&ActiveSession>) -> Vec<SessionLine> {
    sessions
}

/// The active session: the most-recently-modified transcript file within
/// `threshold_secs` of `now`. mtime (not the last assistant event) is the
/// signal because non-assistant records touch the file too, so it reflects
/// live activity more sensitively (spec §5.2).
pub fn active_project(
    mtimes: &[(String, SystemTime)],
    now: DateTime<Utc>,
    threshold_secs: i64,
) -> Option<ActiveSession> {
    let (project, modified) = mtimes.iter().max_by_key(|(_, t)| *t)?;
    let idle = now.signed_duration_since(DateTime::<Utc>::from(*modified));
    (idle.num_seconds() <= threshold_secs).then(|| ActiveSession {
        project: project.clone(),
        idle: idle.max(Duration::zero()),
    })
}

/// Read each discovered file's mtime, dropping any that cannot be stat'd.
/// Split from [`active_project`] so the logic stays pure and testable.
pub fn collect_mtimes(files: &[crate::discover::TranscriptFile]) -> Vec<(String, SystemTime)> {
    files
        .iter()
        .filter_map(|file| {
            let modified = std::fs::metadata(&file.path).ok()?.modified().ok()?;
            Some((file.project.clone(), modified))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::PricingTable;
    use crate::record::{DedupKey, TokenUsage, UsageEvent};
    use chrono::{DateTime, Utc};

    fn event(utc: &str, model: &str, usage: TokenUsage) -> UsageEvent {
        UsageEvent {
            timestamp: utc.parse::<DateTime<Utc>>().unwrap(),
            session_id: Some("s1".into()),
            project: "-Users-v-Projects-gsd".into(),
            model: model.into(),
            usage,
            cost_usd: None,
            cost: rust_decimal::Decimal::ZERO,
            dedup_key: DedupKey::Uuid(format!("{utc}-{model}-{}", usage.output)),
        }
    }

    const U: TokenUsage = TokenUsage {
        input: 100,
        output: 200,
        cache_write_5m: 50,
        cache_write_1h: 10,
        cache_read: 1_000,
    };

    fn priced(model: &str, utc: &str, usage: TokenUsage) -> UsageEvent {
        let table = PricingTable::embedded();
        let mut e = event(utc, model, usage);
        e.cost = crate::cost::Coster::new(&table, crate::cost::CostMode::Calculate).cost(&e);
        e
    }

    fn secs_before(now: DateTime<Utc>, secs: i64) -> SystemTime {
        (now - Duration::seconds(secs)).into()
    }

    #[test]
    fn active_project_picks_the_newest_within_threshold() {
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let mtimes = vec![
            ("proj-old".to_string(), secs_before(now, 200)),
            ("proj-new".to_string(), secs_before(now, 30)),
        ];
        let active = active_project(&mtimes, now, ACTIVE_THRESHOLD_SECS).unwrap();
        assert_eq!(active.project, "proj-new");
        assert_eq!(active.idle.num_seconds(), 30);
    }

    #[test]
    fn active_project_is_none_when_all_files_are_stale() {
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let mtimes = vec![("proj".to_string(), secs_before(now, 600))];
        assert!(active_project(&mtimes, now, ACTIVE_THRESHOLD_SECS).is_none());
    }

    #[test]
    fn active_project_is_none_with_no_files() {
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        assert!(active_project(&[], now, ACTIVE_THRESHOLD_SECS).is_none());
    }

    #[test]
    fn collect_mtimes_reads_each_files_project_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let files = vec![crate::discover::TranscriptFile {
            project: "-Users-v-Projects-gsd".into(),
            path,
        }];
        let mtimes = collect_mtimes(&files);
        assert_eq!(mtimes.len(), 1);
        assert_eq!(mtimes[0].0, "-Users-v-Projects-gsd");
    }

    #[test]
    fn today_totals_cover_only_the_local_day_of_now() {
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let events = vec![
            priced("claude-opus-4-8", "2026-07-04T09:00:00Z", U), // today
            priced("claude-opus-4-8", "2026-07-03T09:00:00Z", U), // yesterday, excluded
        ];
        let state = DashboardState::derive(events, &[], now, chrono_tz::UTC, &table);
        assert_eq!(state.today.totals.output, 200);
        assert_eq!(state.today.totals.input, 100);
        // hit rate = 1000 / (100+50+10+1000)
        assert!(state.today.hit_rate.is_some());
    }

    #[test]
    fn models_line_carries_tokens_cost_and_hit_rate() {
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let events = vec![
            priced("claude-opus-4-8", "2026-07-04T09:00:00Z", U),
            priced("claude-sonnet-5", "2026-07-04T10:00:00Z", U),
        ];
        let state = DashboardState::derive(events, &[], now, chrono_tz::UTC, &table);
        assert_eq!(state.models.len(), 2);
        assert!(state.models.iter().all(|m| m.hit_rate.is_some()));
        assert!(
            state
                .models
                .iter()
                .all(|m| m.cost > rust_decimal::Decimal::ZERO)
        );
    }

    #[test]
    fn burn_rate_is_last_ten_minutes_bucketed_by_minute() {
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let tokens = TokenUsage {
            input: 60,
            output: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 0,
        };
        let events = vec![
            event("2026-07-04T11:59:30Z", "m", tokens), // 0 min ago -> per_minute[9]
            event("2026-07-04T11:51:00Z", "m", tokens), // 9 min ago -> per_minute[0]
            event("2026-07-04T11:40:00Z", "m", tokens), // 20 min ago -> excluded
        ];
        let burn = burn_rate(&events, now);
        assert_eq!(burn.per_minute[9], 60);
        assert_eq!(burn.per_minute[0], 60);
        // 120 tokens in the window / 10 min
        assert!((burn.tokens_per_min - 12.0).abs() < 1e-9);
    }

    #[test]
    fn empty_events_yield_a_zero_state() {
        let table = PricingTable::embedded();
        let now: DateTime<Utc> = "2026-07-04T12:00:00Z".parse().unwrap();
        let state = DashboardState::derive(vec![], &[], now, chrono_tz::UTC, &table);
        assert_eq!(state.today.totals, crate::aggregate::Totals::default());
        assert_eq!(state.today.hit_rate, None);
        assert!(state.models.is_empty());
        assert_eq!(state.burn.per_minute, [0u64; 10]);
        assert_eq!(state.generated_at, now);
    }
}
