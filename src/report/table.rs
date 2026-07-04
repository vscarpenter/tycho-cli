//! Human-readable tables via `comfy-table`.

use crate::aggregate::{DailyReport, Totals};

/// Render the daily report as a table: one row per day, a totals row last.
/// Cache writes are shown combined (the 5m/1h split lives in `--json` and,
/// later, the `cache` report).
pub fn daily(report: &DailyReport) -> String {
    let mut table = comfy_table::Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL_CONDENSED);
    table.set_header(vec![
        "Date",
        "Input",
        "Output",
        "Cache Write",
        "Cache Read",
        "Total Tokens",
    ]);
    for day in &report.days {
        table.add_row(totals_row(day.date.to_string(), &day.totals));
    }
    table.add_row(totals_row("Total".to_owned(), &report.total));
    table.to_string()
}

fn totals_row(label: String, totals: &Totals) -> Vec<String> {
    vec![
        label,
        group_thousands(totals.input),
        group_thousands(totals.output),
        group_thousands(cache_write(totals)),
        group_thousands(totals.cache_read),
        group_thousands(totals.total()),
    ]
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
            },
        }
    }

    #[test]
    fn renders_one_row_per_day_plus_a_totals_row() {
        let rendered = daily(&sample_report());
        assert!(rendered.contains("2026-07-01"));
        assert!(rendered.contains("2026-07-02"));
        assert!(rendered.contains("Total"));
    }

    #[test]
    fn shows_thousands_separators_and_combined_cache_write() {
        let rendered = daily(&sample_report());
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
        let rendered = daily(&sample_report());
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
}
