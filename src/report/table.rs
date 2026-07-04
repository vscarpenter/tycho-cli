//! Human-readable tables via `comfy-table`.

use chrono_tz::Tz;

use crate::aggregate::{
    DailyReport, ModelsReport, MonthlyReport, ProjectsReport, SessionsReport, Totals,
};
use crate::scan::DoctorReport;

/// Render the daily report as a table: one row per day, a totals row last.
/// Cache writes are shown combined (the 5m/1h split lives in `--json` and
/// the `cache` report).
pub fn daily(report: &DailyReport, precise: bool) -> String {
    let mut table = new_table([
        "Date",
        "Input",
        "Output",
        "Cache Write",
        "Cache Read",
        "Total Tokens",
        "Cost (USD)",
    ]);
    for day in &report.days {
        table.add_row(totals_row(day.date.to_string(), &day.totals, precise));
    }
    table.add_row(totals_row("Total".to_owned(), &report.total, precise));
    table.to_string()
}

/// `$12.34`, or `$12.3456` with `--precise`.
fn money(cost: rust_decimal::Decimal, precise: bool) -> String {
    let places = if precise { 4 } else { 2 };
    format!(
        "${}",
        cost.round_dp_with_strategy(places, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    )
}

fn new_table<const N: usize>(header: [&str; N]) -> comfy_table::Table {
    let mut table = comfy_table::Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL_CONDENSED);
    table.set_header(header.to_vec());
    table
}

/// The five token columns shared by most reports.
fn token_columns(totals: &Totals) -> [String; 5] {
    [
        group_thousands(totals.input),
        group_thousands(totals.output),
        group_thousands(cache_write(totals)),
        group_thousands(totals.cache_read),
        group_thousands(totals.total()),
    ]
}

fn totals_row(label: String, totals: &Totals, precise: bool) -> Vec<String> {
    std::iter::once(label)
        .chain(token_columns(totals))
        .chain(std::iter::once(money(totals.cost, precise)))
        .collect()
}

/// Format a token count with thousands separators: `1234567` → `1,234,567`.
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn cache_write(totals: &Totals) -> u64 {
    totals.cache_write_5m + totals.cache_write_1h
}

/// Render the monthly report: one row per month, a totals row last.
pub fn monthly(report: &MonthlyReport, precise: bool) -> String {
    let mut table = new_table([
        "Month",
        "Input",
        "Output",
        "Cache Write",
        "Cache Read",
        "Total Tokens",
        "Cost (USD)",
    ]);
    for month in &report.months {
        table.add_row(totals_row(month.month.clone(), &month.totals, precise));
    }
    table.add_row(totals_row("Total".to_owned(), &report.total, precise));
    table.to_string()
}

/// Render the sessions report. Session ids are shortened to eight
/// characters for width; `--json`/`--csv` carry the full id. The footer
/// totals cover every matching session, not just the shown rows.
pub fn sessions(report: &SessionsReport, tz: Tz, precise: bool) -> String {
    let mut table = new_table([
        "Session",
        "Project",
        "Start",
        "Duration",
        "Models",
        "Total Tokens",
        "Cost (USD)",
    ]);
    for session in &report.sessions {
        table.add_row(vec![
            session.session_id.chars().take(8).collect(),
            session.project.clone(),
            session
                .start
                .with_timezone(&tz)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            human_duration(session.duration()),
            session.models.join(", "),
            group_thousands(session.totals.total()),
            money(session.totals.cost, precise),
        ]);
    }
    table.add_row(vec![
        format!("Total ({} sessions)", report.matching_sessions),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        group_thousands(report.total.total()),
        money(report.total.cost, precise),
    ]);
    table.to_string()
}

/// Render the projects rollup, largest first.
pub fn projects(report: &ProjectsReport, tz: Tz, precise: bool) -> String {
    let mut table = new_table([
        "Project",
        "Sessions",
        "Last Activity",
        "Input",
        "Output",
        "Cache Write",
        "Cache Read",
        "Total Tokens",
        "Cost (USD)",
    ]);
    for project in &report.projects {
        let mut row = vec![
            project.project.clone(),
            group_thousands(project.sessions),
            project
                .last_activity
                .with_timezone(&tz)
                .format("%Y-%m-%d")
                .to_string(),
        ];
        row.extend(token_columns(&project.totals));
        row.push(money(project.totals.cost, precise));
        table.add_row(row);
    }
    let mut total_row = vec!["Total".to_owned(), String::new(), String::new()];
    total_row.extend(token_columns(&report.total));
    total_row.push(money(report.total.cost, precise));
    table.add_row(total_row);
    table.to_string()
}

/// Render the models rollup, largest first.
pub fn models(report: &ModelsReport, precise: bool) -> String {
    let mut table = new_table([
        "Model",
        "Input",
        "Output",
        "Cache Write",
        "Cache Read",
        "Total Tokens",
        "Cost (USD)",
    ]);
    for model in &report.models {
        table.add_row(totals_row(model.model.clone(), &model.totals, precise));
    }
    table.add_row(totals_row("Total".to_owned(), &report.total, precise));
    table.to_string()
}

/// Render the doctor data-health report as key/value rows.
pub fn doctor(report: &DoctorReport) -> String {
    let mut table = new_table(["Check", "Value"]);
    for root in &report.roots {
        let status = if root.exists {
            root.path.display().to_string()
        } else {
            format!("{} (missing)", root.path.display())
        };
        table.add_row(vec!["Search root".to_owned(), status]);
    }
    let stats = &report.summary.stats;
    let span = match report.date_span {
        Some((first, last)) => format!("{} → {}", first.date_naive(), last.date_naive()),
        None => "(no events)".to_owned(),
    };
    let rows: [(&str, String); 14] = [
        (
            "Files scanned",
            group_thousands(report.summary.files_scanned),
        ),
        (
            "Files unreadable",
            group_thousands(report.summary.files_unreadable),
        ),
        (
            "Bytes on disk",
            group_thousands(report.summary.bytes_scanned),
        ),
        ("Lines read", group_thousands(stats.lines)),
        ("Usage events", group_thousands(stats.events)),
        (
            "Duplicates collapsed",
            group_thousands(report.summary.duplicates_collapsed),
        ),
        ("Malformed lines", group_thousands(stats.malformed)),
        ("Other record types", group_thousands(stats.not_assistant)),
        ("Missing usage", group_thousands(stats.missing_usage)),
        (
            "Missing timestamp",
            group_thousands(stats.missing_timestamp),
        ),
        ("Missing model", group_thousands(stats.missing_model)),
        ("Missing identity", group_thousands(stats.missing_identity)),
        (
            "Synthetic (API error) records",
            group_thousands(stats.synthetic),
        ),
        ("Date span", span),
    ];
    for (label, value) in rows {
        table.add_row(vec![label.to_owned(), value]);
    }
    table.add_row(vec!["Models observed".to_owned(), report.models.join("\n")]);
    let unpriced = if report.unpriced_models.is_empty() {
        "(none)".to_owned()
    } else {
        report.unpriced_models.join("\n")
    };
    table.add_row(vec!["Models without pricing".to_owned(), unpriced]);
    table.to_string()
}

/// `2d 2h` / `2h 30m` style humanization: two most significant units.
fn human_duration(duration: chrono::Duration) -> String {
    let secs = duration.num_seconds().max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{DailyReport, DayTotals};
    use chrono::NaiveDate;

    fn sample_report() -> DailyReport {
        let day = |d: u32, output: u64| DayTotals {
            date: NaiveDate::from_ymd_opt(2026, 7, d).unwrap(),
            totals: Totals {
                input: 1_234,
                output,
                cache_write_5m: 5_521,
                cache_write_1h: 1_000,
                cache_read: 815_193,
                cost: "1.25".parse().unwrap(),
            },
        };
        DailyReport {
            days: vec![day(1, 4_260), day(2, 9_000)],
            total: Totals {
                input: 2_468,
                output: 13_260,
                cache_write_5m: 11_042,
                cache_write_1h: 2_000,
                cache_read: 1_630_386,
                cost: "2.50".parse().unwrap(),
            },
        }
    }

    #[test]
    fn renders_one_row_per_day_plus_a_totals_row() {
        let rendered = daily(&sample_report(), false);
        assert!(rendered.contains("2026-07-01"));
        assert!(rendered.contains("2026-07-02"));
        assert!(rendered.contains("Total"));
    }

    #[test]
    fn shows_thousands_separators_and_combined_cache_write() {
        let rendered = daily(&sample_report(), false);
        assert!(
            rendered.contains("815,193"),
            "cache read column:\n{rendered}"
        );
        assert!(
            rendered.contains("6,521"),
            "cache write = 5m + 1h:\n{rendered}"
        );
        assert!(rendered.contains("1,630,386"), "totals row:\n{rendered}");
    }

    #[test]
    fn has_the_expected_column_headers() {
        let rendered = daily(&sample_report(), false);
        for header in [
            "Date",
            "Input",
            "Output",
            "Cache Write",
            "Cache Read",
            "Total Tokens",
        ] {
            assert!(
                rendered.contains(header),
                "missing header {header:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn groups_thousands_correctly() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1,000");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    fn totals(output: u64) -> Totals {
        Totals {
            input: 100,
            output,
            cache_write_5m: 10,
            cache_write_1h: 20,
            cache_read: 1_000,
            cost: "0.5".parse().unwrap(),
        }
    }

    #[test]
    fn monthly_renders_months_and_total() {
        let report = MonthlyReport {
            months: vec![crate::aggregate::MonthTotals {
                month: "2026-07".into(),
                totals: totals(5_000),
            }],
            total: totals(5_000),
        };
        let rendered = monthly(&report, false);
        assert!(rendered.contains("Month"));
        assert!(rendered.contains("2026-07"));
        assert!(rendered.contains("5,000"));
        assert!(rendered.contains("Total"));
    }

    #[test]
    fn sessions_renders_span_duration_and_models() {
        let report = SessionsReport {
            sessions: vec![crate::aggregate::SessionTotals {
                session_id: "0123456789abcdef".into(),
                project: "-Users-v-Projects-gsd".into(),
                start: "2026-07-02T10:00:00Z".parse().unwrap(),
                end: "2026-07-02T12:30:00Z".parse().unwrap(),
                models: vec!["opus".into(), "sonnet".into()],
                totals: totals(7),
            }],
            matching_sessions: 5,
            total: totals(7),
        };
        let rendered = sessions(&report, chrono_tz::UTC, false);
        assert!(rendered.contains("01234567"), "shortened id:\n{rendered}");
        assert!(!rendered.contains("0123456789abcdef"));
        assert!(rendered.contains("2026-07-02 10:00"));
        assert!(rendered.contains("2h 30m"));
        assert!(rendered.contains("opus, sonnet"));
        assert!(rendered.contains("Total (5 sessions)"));
    }

    #[test]
    fn projects_renders_counts_and_last_activity() {
        let report = ProjectsReport {
            projects: vec![crate::aggregate::ProjectTotals {
                project: "-Users-v-Projects-gsd".into(),
                sessions: 3,
                last_activity: "2026-07-03T04:30:00Z".parse().unwrap(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let rendered = projects(&report, chrono_tz::UTC, false);
        assert!(rendered.contains("-Users-v-Projects-gsd"));
        assert!(rendered.contains("Sessions"));
        assert!(rendered.contains("2026-07-03"));
    }

    #[test]
    fn models_renders_rollup() {
        let report = ModelsReport {
            models: vec![crate::aggregate::ModelTotals {
                model: "claude-opus-4-8".into(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let rendered = models(&report, false);
        assert!(rendered.contains("claude-opus-4-8"));
        assert!(rendered.contains("Total"));
    }

    #[test]
    fn doctor_renders_roots_counters_and_span() {
        let report = DoctorReport {
            roots: vec![
                crate::scan::RootStatus {
                    path: "/home/v/.claude/projects".into(),
                    exists: true,
                },
                crate::scan::RootStatus {
                    path: "/missing".into(),
                    exists: false,
                },
            ],
            summary: crate::scan::ScanSummary {
                files_scanned: 4,
                files_unreadable: 1,
                duplicates_collapsed: 2,
                bytes_scanned: 1_048_576,
                stats: crate::record::ParseStats {
                    lines: 100,
                    events: 50,
                    malformed: 3,
                    ..Default::default()
                },
            },
            date_span: Some((
                "2026-05-17T00:00:00Z".parse().unwrap(),
                "2026-07-04T00:00:00Z".parse().unwrap(),
            )),
            models: vec!["claude-opus-4-8".into()],
            unpriced_models: vec!["mystery-model".into()],
        };
        let rendered = doctor(&report);
        assert!(rendered.contains("/home/v/.claude/projects"));
        assert!(
            rendered.contains("missing"),
            "absent root marked:\n{rendered}"
        );
        assert!(rendered.contains("1,048,576"));
        assert!(rendered.contains("2026-05-17"));
        assert!(rendered.contains("claude-opus-4-8"));
        assert!(rendered.contains("3"), "malformed count shown");
    }

    #[test]
    fn humanizes_durations_at_two_units() {
        use chrono::Duration;
        assert_eq!(human_duration(Duration::seconds(59)), "59s");
        assert_eq!(human_duration(Duration::minutes(5)), "5m");
        assert_eq!(human_duration(Duration::minutes(150)), "2h 30m");
        assert_eq!(human_duration(Duration::hours(50)), "2d 2h");
    }
}
