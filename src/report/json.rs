//! Machine-readable JSON output — the scripting API.
//!
//! The shape below is a documented contract (`docs/json-schema.md`):
//! adding fields is allowed, renaming or removing them is a breaking
//! change. Token counts keep the 5m/1h cache-write split that the human
//! table combines. The nested `tokens` object leaves room for a sibling
//! `cost` object in Phase 3 without breaking consumers.

use chrono::SecondsFormat;
use serde::Serialize;

use crate::aggregate::{
    DailyReport, ModelsReport, MonthlyReport, ProjectsReport, SessionsReport, Totals,
};
use crate::scan::DoctorReport;

fn rfc3339(ts: chrono::DateTime<chrono::Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// A struct of plain fields cannot fail to serialize; fall back to "{}"
// rather than panicking if serde_json ever disagrees.
fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned())
}

/// One serialized token block (with its cost).
#[derive(Serialize)]
struct TokensOut {
    input: u64,
    output: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
    total: u64,
    cost_usd: f64,
}

impl From<&Totals> for TokensOut {
    fn from(totals: &Totals) -> Self {
        use rust_decimal::prelude::ToPrimitive;
        Self {
            input: totals.input,
            output: totals.output,
            cache_write_5m: totals.cache_write_5m,
            cache_write_1h: totals.cache_write_1h,
            cache_read: totals.cache_read,
            total: totals.total(),
            cost_usd: totals.cost.to_f64().unwrap_or(0.0),
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
    to_json(&out)
}

#[derive(Serialize)]
struct MonthOut {
    month: String,
    tokens: TokensOut,
}

#[derive(Serialize)]
struct MonthlyOut {
    command: &'static str,
    timezone: String,
    months: Vec<MonthOut>,
    totals: TokensOut,
}

/// Render the monthly report as pretty-printed JSON.
pub fn monthly(report: &MonthlyReport, timezone: &str) -> String {
    let out = MonthlyOut {
        command: "monthly",
        timezone: timezone.to_owned(),
        months: report
            .months
            .iter()
            .map(|month| MonthOut {
                month: month.month.clone(),
                tokens: TokensOut::from(&month.totals),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct SessionOut {
    session_id: String,
    project: String,
    start: String,
    end: String,
    duration_seconds: i64,
    models: Vec<String>,
    tokens: TokensOut,
}

#[derive(Serialize)]
struct SessionsOut {
    command: &'static str,
    timezone: String,
    matching_sessions: usize,
    sessions: Vec<SessionOut>,
    totals: TokensOut,
}

/// Render the sessions report as pretty-printed JSON. Instants are RFC3339
/// UTC regardless of the report timezone (which governs date bucketing).
pub fn sessions(report: &SessionsReport, timezone: &str) -> String {
    let out = SessionsOut {
        command: "sessions",
        timezone: timezone.to_owned(),
        matching_sessions: report.matching_sessions,
        sessions: report
            .sessions
            .iter()
            .map(|session| SessionOut {
                session_id: session.session_id.clone(),
                project: session.project.clone(),
                start: rfc3339(session.start),
                end: rfc3339(session.end),
                duration_seconds: session.duration().num_seconds(),
                models: session.models.clone(),
                tokens: TokensOut::from(&session.totals),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct ProjectOut {
    project: String,
    sessions: u64,
    last_activity: String,
    tokens: TokensOut,
}

#[derive(Serialize)]
struct ProjectsOut {
    command: &'static str,
    timezone: String,
    projects: Vec<ProjectOut>,
    totals: TokensOut,
}

/// Render the projects rollup as pretty-printed JSON.
pub fn projects(report: &ProjectsReport, timezone: &str) -> String {
    let out = ProjectsOut {
        command: "projects",
        timezone: timezone.to_owned(),
        projects: report
            .projects
            .iter()
            .map(|project| ProjectOut {
                project: project.project.clone(),
                sessions: project.sessions,
                last_activity: rfc3339(project.last_activity),
                tokens: TokensOut::from(&project.totals),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct ModelOut {
    model: String,
    tokens: TokensOut,
}

#[derive(Serialize)]
struct ModelsOut {
    command: &'static str,
    timezone: String,
    models: Vec<ModelOut>,
    totals: TokensOut,
}

/// Render the models rollup as pretty-printed JSON.
pub fn models(report: &ModelsReport, timezone: &str) -> String {
    let out = ModelsOut {
        command: "models",
        timezone: timezone.to_owned(),
        models: report
            .models
            .iter()
            .map(|model| ModelOut {
                model: model.model.clone(),
                tokens: TokensOut::from(&model.totals),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct CacheRowOut {
    model: String,
    tokens: TokensOut,
    hit_rate: Option<f64>,
    actual_cost_usd: f64,
    counterfactual_cost_usd: f64,
    savings_usd: f64,
    leverage: Option<f64>,
    /// Extra cost from 1-hour cache writes over the 5-minute rate. The
    /// token split itself is already in `tokens.cache_write_{5m,1h}`.
    ttl_premium_usd: f64,
}

impl From<&crate::cache::CacheEconomics> for CacheRowOut {
    fn from(row: &crate::cache::CacheEconomics) -> Self {
        use rust_decimal::prelude::ToPrimitive;
        Self {
            model: row.model.clone(),
            tokens: TokensOut::from(&row.totals),
            hit_rate: row.hit_rate.and_then(|r| r.to_f64()),
            actual_cost_usd: row.actual_cost.to_f64().unwrap_or(0.0),
            counterfactual_cost_usd: row.counterfactual_cost.to_f64().unwrap_or(0.0),
            savings_usd: row.savings.to_f64().unwrap_or(0.0),
            leverage: row.leverage.and_then(|l| l.to_f64()),
            ttl_premium_usd: row.ttl_premium.to_f64().unwrap_or(0.0),
        }
    }
}

#[derive(Serialize)]
struct CacheOut {
    command: &'static str,
    timezone: String,
    models: Vec<CacheRowOut>,
    totals: CacheRowOut,
}

/// Render the cache-economics report as pretty-printed JSON.
pub fn cache(report: &crate::cache::CacheReport, timezone: &str) -> String {
    let out = CacheOut {
        command: "cache",
        timezone: timezone.to_owned(),
        models: report.models.iter().map(CacheRowOut::from).collect(),
        totals: CacheRowOut::from(&report.total),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct RootOut {
    path: String,
    exists: bool,
}

#[derive(Serialize)]
struct FilesOut {
    scanned: u64,
    unreadable: u64,
    bytes: u64,
}

#[derive(Serialize)]
struct LinesOut {
    total: u64,
    events: u64,
    malformed: u64,
    not_assistant: u64,
    missing_usage: u64,
    missing_timestamp: u64,
    missing_model: u64,
    missing_identity: u64,
    synthetic: u64,
    codex_model_backfilled: u64,
}

#[derive(Serialize)]
struct SpanOut {
    first: String,
    last: String,
}

#[derive(Serialize)]
struct DoctorOut {
    command: &'static str,
    roots: Vec<RootOut>,
    files: FilesOut,
    lines: LinesOut,
    duplicates_collapsed: u64,
    date_span: Option<SpanOut>,
    models: Vec<String>,
    unpriced_models: Vec<String>,
    zero_rated_models: Vec<String>,
    local_models: Vec<String>,
    /// Always present (empty off WSL) so the contract stays stable.
    unscanned_windows_roots: Vec<String>,
}

/// Render the doctor data-health report as pretty-printed JSON.
pub fn doctor(report: &DoctorReport) -> String {
    let stats = &report.summary.stats;
    let out = DoctorOut {
        command: "doctor",
        roots: report
            .roots
            .iter()
            .map(|root| RootOut {
                path: root.path.display().to_string(),
                exists: root.exists,
            })
            .collect(),
        files: FilesOut {
            scanned: report.summary.files_scanned,
            unreadable: report.summary.files_unreadable,
            bytes: report.summary.bytes_scanned,
        },
        lines: LinesOut {
            total: stats.lines,
            events: stats.events,
            malformed: stats.malformed,
            not_assistant: stats.not_assistant,
            missing_usage: stats.missing_usage,
            missing_timestamp: stats.missing_timestamp,
            missing_model: stats.missing_model,
            missing_identity: stats.missing_identity,
            synthetic: stats.synthetic,
            codex_model_backfilled: stats.codex_model_backfilled,
        },
        duplicates_collapsed: report.summary.duplicates_collapsed,
        date_span: report.date_span.map(|(first, last)| SpanOut {
            first: rfc3339(first),
            last: rfc3339(last),
        }),
        models: report.models.clone(),
        unpriced_models: report.unpriced_models.clone(),
        zero_rated_models: report.zero_rated_models.clone(),
        local_models: report.local_models.clone(),
        unscanned_windows_roots: report
            .unscanned_windows_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct BurnOut {
    tokens_per_min: f64,
    cost_per_hour_usd: f64,
    per_minute: [u64; 10],
}

#[derive(Serialize)]
struct ModelLineOut {
    model: String,
    tokens: u64,
    cost_usd: f64,
    hit_rate: Option<f64>,
}

#[derive(Serialize)]
struct SessionLineOut {
    session_id: String,
    project: String,
    tokens: u64,
    cost_usd: f64,
    last_activity: String,
    active: bool,
}

#[derive(Serialize)]
struct ActiveOut {
    project: String,
    idle_seconds: i64,
}

#[derive(Serialize)]
struct LiveOut {
    command: &'static str,
    timezone: String,
    generated_at: String,
    today: CacheRowOut,
    burn: BurnOut,
    models: Vec<ModelLineOut>,
    sessions: Vec<SessionLineOut>,
    active: Option<ActiveOut>,
}

/// Render one live dashboard snapshot as pretty-printed JSON. This is what
/// `tycho live --json` (or `live` with a non-TTY stdout) prints.
pub fn live(state: &crate::tui::state::DashboardState, timezone: &str) -> String {
    use rust_decimal::prelude::ToPrimitive;
    let out = LiveOut {
        command: "live",
        timezone: timezone.to_owned(),
        generated_at: rfc3339(state.generated_at),
        today: CacheRowOut::from(&state.today),
        burn: BurnOut {
            tokens_per_min: state.burn.tokens_per_min,
            cost_per_hour_usd: state.burn.cost_per_hour.to_f64().unwrap_or(0.0),
            per_minute: state.burn.per_minute,
        },
        models: state
            .models
            .iter()
            .map(|m| ModelLineOut {
                model: m.model.clone(),
                tokens: m.tokens,
                cost_usd: m.cost.to_f64().unwrap_or(0.0),
                hit_rate: m.hit_rate.and_then(|r| r.to_f64()),
            })
            .collect(),
        sessions: state
            .sessions
            .iter()
            .map(|s| SessionLineOut {
                session_id: s.session_id.clone(),
                project: s.project.clone(),
                tokens: s.tokens,
                cost_usd: s.cost.to_f64().unwrap_or(0.0),
                last_activity: rfc3339(s.last_activity),
                active: s.active,
            })
            .collect(),
        active: state.active.as_ref().map(|a| ActiveOut {
            project: a.project.clone(),
            idle_seconds: a.idle.num_seconds(),
        }),
    };
    to_json(&out)
}

#[derive(Serialize)]
struct ProjectionOut {
    projected_cost_usd: f64,
    projected_tokens: u64,
    burn_tokens_per_min: f64,
    remaining_seconds: i64,
}

#[derive(Serialize)]
struct BlockOut {
    start: String,
    end: String,
    first_activity: String,
    last_activity: String,
    tokens: TokensOut,
    models: Vec<String>,
    active: bool,
    projection: Option<ProjectionOut>,
}

#[derive(Serialize)]
struct BlocksOut {
    command: &'static str,
    timezone: String,
    blocks: Vec<BlockOut>,
    totals: TokensOut,
}

/// Render the billing-blocks report as pretty-printed JSON.
pub fn blocks(report: &crate::blocks::BlocksReport, timezone: &str) -> String {
    use rust_decimal::prelude::ToPrimitive;
    let out = BlocksOut {
        command: "blocks",
        timezone: timezone.to_owned(),
        blocks: report
            .blocks
            .iter()
            .map(|b| BlockOut {
                start: rfc3339(b.start),
                end: rfc3339(b.end),
                first_activity: rfc3339(b.first_activity),
                last_activity: rfc3339(b.last_activity),
                tokens: TokensOut::from(&b.totals),
                models: b.models.clone(),
                active: b.projection.is_some(),
                projection: b.projection.as_ref().map(|p| ProjectionOut {
                    projected_cost_usd: p.projected_cost.to_f64().unwrap_or(0.0),
                    projected_tokens: p.projected_tokens,
                    burn_tokens_per_min: p.burn_tokens_per_min,
                    remaining_seconds: p.remaining.num_seconds(),
                }),
            })
            .collect(),
        totals: TokensOut::from(&report.total),
    };
    to_json(&out)
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
                    cost: "0.75".parse().unwrap(),
                },
            }],
            total: Totals {
                input: 2,
                output: 5_301,
                cache_write_5m: 4_000,
                cache_write_1h: 1_521,
                cache_read: 196_377,
                cost: "0.75".parse().unwrap(),
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

    fn totals(output: u64) -> Totals {
        Totals {
            input: 1,
            output,
            cache_write_5m: 2,
            cache_write_1h: 3,
            cache_read: 4,
            cost: "0.5".parse().unwrap(),
        }
    }

    #[test]
    fn monthly_contract() {
        let report = MonthlyReport {
            months: vec![crate::aggregate::MonthTotals {
                month: "2026-07".into(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let value: serde_json::Value = serde_json::from_str(&monthly(&report, "UTC")).unwrap();
        assert_eq!(value["command"], "monthly");
        assert_eq!(value["months"][0]["month"], "2026-07");
        assert_eq!(value["months"][0]["tokens"]["output"], 9);
    }

    #[test]
    fn sessions_contract_keeps_full_id_and_instants() {
        let report = SessionsReport {
            sessions: vec![crate::aggregate::SessionTotals {
                session_id: "0123456789abcdef".into(),
                project: "p".into(),
                start: "2026-07-02T10:00:00Z".parse().unwrap(),
                end: "2026-07-02T12:30:00Z".parse().unwrap(),
                models: vec!["opus".into()],
                totals: totals(9),
            }],
            matching_sessions: 3,
            total: totals(9),
        };
        let value: serde_json::Value =
            serde_json::from_str(&sessions(&report, "America/Chicago")).unwrap();
        assert_eq!(value["command"], "sessions");
        assert_eq!(value["matching_sessions"], 3);
        let s = &value["sessions"][0];
        assert_eq!(s["session_id"], "0123456789abcdef");
        assert_eq!(s["start"], "2026-07-02T10:00:00Z");
        assert_eq!(s["duration_seconds"], 9_000);
        assert_eq!(s["models"][0], "opus");
    }

    #[test]
    fn projects_and_models_contracts() {
        let p = ProjectsReport {
            projects: vec![crate::aggregate::ProjectTotals {
                project: "gsd".into(),
                sessions: 2,
                last_activity: "2026-07-03T04:30:00Z".parse().unwrap(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let value: serde_json::Value = serde_json::from_str(&projects(&p, "UTC")).unwrap();
        assert_eq!(value["projects"][0]["sessions"], 2);
        assert_eq!(
            value["projects"][0]["last_activity"],
            "2026-07-03T04:30:00Z"
        );

        let m = ModelsReport {
            models: vec![crate::aggregate::ModelTotals {
                model: "claude-opus-4-8".into(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let value: serde_json::Value = serde_json::from_str(&models(&m, "UTC")).unwrap();
        assert_eq!(value["models"][0]["model"], "claude-opus-4-8");
    }

    #[test]
    fn live_contract() {
        use crate::tui::state::{ActiveSession, BurnRate, DashboardState, ModelLine, SessionLine};
        let totals = Totals {
            input: 100,
            output: 200,
            cache_write_5m: 50,
            cache_write_1h: 10,
            cache_read: 1_000,
            cost: "0.64".parse().unwrap(),
        };
        let state = DashboardState {
            today: crate::cache::CacheEconomics {
                model: "Total".into(),
                totals,
                hit_rate: Some("0.86".parse().unwrap()),
                actual_cost: "0.64".parse().unwrap(),
                counterfactual_cost: "1.08".parse().unwrap(),
                savings: "0.44".parse().unwrap(),
                leverage: Some("1.68".parse().unwrap()),
                ttl_premium: "0.02".parse().unwrap(),
            },
            burn: BurnRate {
                tokens_per_min: 12.0,
                cost_per_hour: "0.9".parse().unwrap(),
                per_minute: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            },
            models: vec![ModelLine {
                model: "claude-opus-4-8".into(),
                tokens: 1_360,
                cost: "0.64".parse().unwrap(),
                hit_rate: Some("0.86".parse().unwrap()),
            }],
            sessions: vec![SessionLine {
                session_id: "s1".into(),
                project: "gsd".into(),
                tokens: 1_360,
                cost: "0.64".parse().unwrap(),
                last_activity: "2026-07-04T11:59:00Z".parse().unwrap(),
                active: true,
            }],
            active: Some(ActiveSession {
                project: "gsd".into(),
                idle: chrono::Duration::seconds(20),
            }),
            generated_at: "2026-07-04T12:00:00Z".parse().unwrap(),
        };
        let value: serde_json::Value = serde_json::from_str(&live(&state, "UTC")).unwrap();
        assert_eq!(value["command"], "live");
        assert_eq!(value["generated_at"], "2026-07-04T12:00:00Z");
        assert_eq!(value["today"]["tokens"]["total"], 1_360);
        assert_eq!(value["today"]["hit_rate"], 0.86);
        assert_eq!(value["burn"]["tokens_per_min"], 12.0);
        assert_eq!(value["active"]["project"], "gsd");
        assert_eq!(value["models"][0]["model"], "claude-opus-4-8");
        assert_eq!(value["sessions"][0]["active"], true);
    }

    #[test]
    fn blocks_contract() {
        use crate::blocks::{BlockProjection, BlockTotals, BlocksReport};
        let totals = Totals {
            input: 100,
            output: 200,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 0,
            cost: "3.00".parse().unwrap(),
        };
        let report = BlocksReport {
            blocks: vec![BlockTotals {
                start: "2026-07-04T12:00:00Z".parse().unwrap(),
                end: "2026-07-04T17:00:00Z".parse().unwrap(),
                first_activity: "2026-07-04T12:05:00Z".parse().unwrap(),
                last_activity: "2026-07-04T14:20:00Z".parse().unwrap(),
                totals,
                models: vec!["claude-opus-4-8".into()],
                projection: Some(BlockProjection {
                    projected_cost: "6.00".parse().unwrap(),
                    projected_tokens: 600,
                    burn_tokens_per_min: 2.0,
                    remaining: chrono::Duration::minutes(160),
                }),
            }],
            total: totals,
        };
        let value: serde_json::Value = serde_json::from_str(&blocks(&report, "UTC")).unwrap();
        assert_eq!(value["command"], "blocks");
        let b = &value["blocks"][0];
        assert_eq!(b["start"], "2026-07-04T12:00:00Z");
        assert_eq!(b["end"], "2026-07-04T17:00:00Z");
        assert_eq!(b["tokens"]["total"], 300);
        assert_eq!(b["models"][0], "claude-opus-4-8");
        assert_eq!(b["active"], true);
        assert_eq!(b["projection"]["projected_cost_usd"], 6.0);
        assert_eq!(b["projection"]["remaining_seconds"], 9_600);
        assert_eq!(value["totals"]["total"], 300);
    }

    /// The cache report exposes the TTL split (already carried by `tokens`)
    /// plus the premium those 1-hour writes cost.
    #[test]
    fn cache_contract() {
        let row = crate::cache::CacheEconomics {
            model: "claude-fable-5".into(),
            totals: Totals {
                cache_write_5m: 1_000_000,
                cache_write_1h: 2_000_000,
                cache_read: 500,
                ..Totals::default()
            },
            hit_rate: Some("0.5".parse().unwrap()),
            actual_cost: "1.0".parse().unwrap(),
            counterfactual_cost: "2.0".parse().unwrap(),
            savings: "1.0".parse().unwrap(),
            leverage: Some("2.0".parse().unwrap()),
            ttl_premium: "15.0".parse().unwrap(),
        };
        let report = crate::cache::CacheReport {
            models: vec![row.clone()],
            total: crate::cache::CacheEconomics {
                model: "Total".into(),
                ..row
            },
        };
        let value: serde_json::Value = serde_json::from_str(&cache(&report, "UTC")).unwrap();
        assert_eq!(value["command"], "cache");
        assert_eq!(value["models"][0]["tokens"]["cache_write_5m"], 1_000_000);
        assert_eq!(value["models"][0]["tokens"]["cache_write_1h"], 2_000_000);
        assert_eq!(value["models"][0]["ttl_premium_usd"], 15.0);
        assert_eq!(value["totals"]["ttl_premium_usd"], 15.0);
    }

    #[test]
    fn doctor_contract() {
        let report = DoctorReport {
            roots: vec![crate::scan::RootStatus {
                path: "/r".into(),
                exists: true,
            }],
            summary: crate::scan::ScanSummary {
                files_scanned: 4,
                files_unreadable: 1,
                duplicates_collapsed: 2,
                bytes_scanned: 512,
                stats: crate::record::ParseStats {
                    lines: 10,
                    events: 5,
                    malformed: 1,
                    codex_model_backfilled: 3,
                    ..Default::default()
                },
            },
            date_span: Some((
                "2026-05-17T00:00:00Z".parse().unwrap(),
                "2026-07-04T00:00:00Z".parse().unwrap(),
            )),
            models: vec!["m1".into()],
            unpriced_models: vec!["mystery-model".into()],
            zero_rated_models: vec!["gpt-5.2-codex".into()],
            local_models: vec!["qwen3.6:27b".into()],
            unscanned_windows_roots: vec!["/mnt/c/Users/v/.claude/projects".into()],
        };
        let value: serde_json::Value = serde_json::from_str(&doctor(&report)).unwrap();
        assert_eq!(value["command"], "doctor");
        assert_eq!(value["roots"][0]["exists"], true);
        assert_eq!(value["files"]["bytes"], 512);
        assert_eq!(value["lines"]["malformed"], 1);
        assert_eq!(value["lines"]["codex_model_backfilled"], 3);
        assert_eq!(value["duplicates_collapsed"], 2);
        assert_eq!(value["date_span"]["first"], "2026-05-17T00:00:00Z");
        assert_eq!(value["models"][0], "m1");
        assert_eq!(value["zero_rated_models"][0], "gpt-5.2-codex");
        assert_eq!(value["local_models"][0], "qwen3.6:27b");
        assert_eq!(
            value["unscanned_windows_roots"][0],
            "/mnt/c/Users/v/.claude/projects"
        );
    }
}
