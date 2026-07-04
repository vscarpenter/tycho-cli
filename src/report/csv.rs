//! CSV output for `daily`, `monthly`, and `sessions`.
//!
//! Data rows only — no totals row; spreadsheet and script consumers sum
//! for themselves. Column names match the JSON contract's token keys.

use crate::aggregate::{DailyReport, MonthlyReport, SessionsReport, Totals};

/// CSV for the daily report.
pub fn daily(report: &DailyReport) -> String {
    let mut rows = vec![labeled_headers("date")];
    rows.extend(
        report
            .days
            .iter()
            .map(|day| labeled_row(day.date.to_string(), &day.totals)),
    );
    render(rows)
}

/// CSV for the monthly report.
pub fn monthly(report: &MonthlyReport) -> String {
    let mut rows = vec![labeled_headers("month")];
    rows.extend(
        report
            .months
            .iter()
            .map(|month| labeled_row(month.month.clone(), &month.totals)),
    );
    render(rows)
}

/// CSV for the sessions report. Models are joined with `;` so the file
/// stays one row per session.
pub fn sessions(report: &SessionsReport) -> String {
    let mut header: Vec<String> = [
        "session_id",
        "project",
        "start",
        "end",
        "duration_seconds",
        "models",
    ]
    .map(str::to_owned)
    .to_vec();
    header.extend(TOKEN_HEADERS.map(str::to_owned));
    let mut rows = vec![header];
    for session in &report.sessions {
        let mut row = vec![
            session.session_id.clone(),
            session.project.clone(),
            session
                .start
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            session
                .end
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            session.duration().num_seconds().to_string(),
            session.models.join(";"),
        ];
        row.extend(token_fields(&session.totals));
        rows.push(row);
    }
    render(rows)
}

fn labeled_headers(label: &str) -> Vec<String> {
    std::iter::once(label.to_owned())
        .chain(TOKEN_HEADERS.map(str::to_owned))
        .collect()
}

fn labeled_row(label: String, totals: &Totals) -> Vec<String> {
    std::iter::once(label).chain(token_fields(totals)).collect()
}

fn token_fields(totals: &Totals) -> [String; 6] {
    [
        totals.input.to_string(),
        totals.output.to_string(),
        totals.cache_write_5m.to_string(),
        totals.cache_write_1h.to_string(),
        totals.cache_read.to_string(),
        totals.total().to_string(),
    ]
}

const TOKEN_HEADERS: [&str; 6] = [
    "input",
    "output",
    "cache_write_5m",
    "cache_write_1h",
    "cache_read",
    "total",
];

/// Write rows into an in-memory CSV. Writing to a `Vec<u8>` cannot fail,
/// so errors degrade to an empty string instead of panicking.
fn render(rows: Vec<Vec<String>>) -> String {
    let mut writer = csv::Writer::from_writer(Vec::new());
    for row in rows {
        let _ = writer.write_record(&row);
    }
    writer
        .into_inner()
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{DayTotals, MonthTotals, SessionTotals};
    use chrono::NaiveDate;

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
    fn daily_csv_has_headers_and_data_rows_only() {
        let report = DailyReport {
            days: vec![DayTotals {
                date: NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let rendered = daily(&report);
        let lines: Vec<&str> = rendered.trim_end().lines().collect();
        assert_eq!(
            lines[0],
            "date,input,output,cache_write_5m,cache_write_1h,cache_read,total"
        );
        assert_eq!(lines[1], "2026-07-02,1,9,2,3,4,19");
        assert_eq!(lines.len(), 2, "no totals row in CSV");
    }

    #[test]
    fn monthly_csv_uses_month_column() {
        let report = MonthlyReport {
            months: vec![MonthTotals {
                month: "2026-07".into(),
                totals: totals(9),
            }],
            total: totals(9),
        };
        let rendered = monthly(&report);
        assert!(rendered.starts_with("month,input"));
        assert!(rendered.contains("2026-07,1,9,2,3,4,19"));
    }

    #[test]
    fn sessions_csv_joins_models_and_keeps_full_id() {
        let report = SessionsReport {
            sessions: vec![SessionTotals {
                session_id: "0123456789abcdef".into(),
                project: "-Users-v-Projects-gsd".into(),
                start: "2026-07-02T10:00:00Z".parse().unwrap(),
                end: "2026-07-02T12:30:00Z".parse().unwrap(),
                models: vec!["opus".into(), "sonnet".into()],
                totals: totals(9),
            }],
            matching_sessions: 1,
            total: totals(9),
        };
        let rendered = sessions(&report);
        let lines: Vec<&str> = rendered.trim_end().lines().collect();
        assert_eq!(
            lines[0],
            "session_id,project,start,end,duration_seconds,models,input,output,cache_write_5m,cache_write_1h,cache_read,total"
        );
        assert!(lines[1].starts_with("0123456789abcdef,-Users-v-Projects-gsd,"));
        assert!(lines[1].contains("2026-07-02T10:00:00Z,2026-07-02T12:30:00Z,9000,opus;sonnet,"));
    }
}
