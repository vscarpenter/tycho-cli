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
        },
        duplicates_collapsed: report.summary.duplicates_collapsed,
        date_span: report.date_span.map(|(first, last)| SpanOut {
            first: rfc3339(first),
            last: rfc3339(last),
        }),
        models: report.models.clone(),
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

    fn totals(output: u64) -> Totals {
        Totals {
            input: 1,
            output,
            cache_write_5m: 2,
            cache_write_1h: 3,
            cache_read: 4,
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
                    ..Default::default()
                },
            },
            date_span: Some((
                "2026-05-17T00:00:00Z".parse().unwrap(),
                "2026-07-04T00:00:00Z".parse().unwrap(),
            )),
            models: vec!["m1".into()],
        };
        let value: serde_json::Value = serde_json::from_str(&doctor(&report)).unwrap();
        assert_eq!(value["command"], "doctor");
        assert_eq!(value["roots"][0]["exists"], true);
        assert_eq!(value["files"]["bytes"], 512);
        assert_eq!(value["lines"]["malformed"], 1);
        assert_eq!(value["duplicates_collapsed"], 2);
        assert_eq!(value["date_span"]["first"], "2026-05-17T00:00:00Z");
        assert_eq!(value["models"][0], "m1");
    }
}
