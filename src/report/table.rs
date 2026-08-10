//! Human-readable tables via `comfy-table`.

use chrono_tz::Tz;

use crate::aggregate::{
    DailyReport, ModelsReport, MonthlyReport, ProjectsReport, SessionsReport, Totals,
};
use crate::blocks::BLOCK_HOURS;
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

/// Render the cache-economics report: the headline sentence, then one row
/// per model plus the total.
pub fn cache(report: &crate::cache::CacheReport, precise: bool) -> String {
    let total = &report.total;
    let mut headline = match total.leverage {
        Some(leverage) => format!(
            "Your effective cost was {}. Without prompt caching it would have been {}.\nCaching saved you {} ({} leverage).\n",
            money(total.actual_cost, precise),
            money(total.counterfactual_cost, precise),
            money(total.savings, precise),
            leverage_x(Some(leverage)),
        ),
        None => "No costed usage in this window.\n".to_owned(),
    };
    // The guard also makes the divisor non-zero, so no separate check.
    if total.totals.cache_write_1h > 0 {
        let share = rust_decimal::Decimal::from(total.totals.cache_write_1h)
            * rust_decimal::Decimal::from(100u8)
            / rust_decimal::Decimal::from(cache_write(&total.totals));
        headline.push_str(&format!(
            "{}% of your cache writes used the 1-hour TTL, costing {} more than the 5-minute rate.\n",
            share.round_dp_with_strategy(1, rust_decimal::RoundingStrategy::MidpointAwayFromZero),
            money(total.ttl_premium, precise),
        ));
    }

    let mut table = new_table([
        "Model",
        "Cache Read",
        "Write 5m",
        "Write 1h",
        "Hit Rate",
        "Actual Cost",
        "No-Cache Cost",
        "Savings",
        "Leverage",
    ]);
    for row in report.models.iter().chain(std::iter::once(total)) {
        table.add_row(vec![
            row.model.clone(),
            group_thousands(row.totals.cache_read),
            group_thousands(row.totals.cache_write_5m),
            group_thousands(row.totals.cache_write_1h),
            percent(row.hit_rate),
            money(row.actual_cost, precise),
            money(row.counterfactual_cost, precise),
            money(row.savings, precise),
            leverage_x(row.leverage),
        ]);
    }
    format!("{headline}\n{table}")
}

/// `86.2%`, or `-` when undefined.
fn percent(rate: Option<rust_decimal::Decimal>) -> String {
    match rate {
        Some(rate) => format!(
            "{}%",
            (rate * rust_decimal::Decimal::from(100u8))
                .round_dp_with_strategy(1, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        ),
        None => "-".to_owned(),
    }
}

/// `1.7x`, or `-` when undefined.
fn leverage_x(leverage: Option<rust_decimal::Decimal>) -> String {
    match leverage {
        Some(leverage) => format!(
            "{}x",
            leverage
                .round_dp_with_strategy(1, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        ),
        None => "-".to_owned(),
    }
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
    let rows: [(&str, String); 15] = [
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
        (
            "Codex models backfilled",
            group_thousands(stats.codex_model_backfilled),
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
    let zero_rated = if report.zero_rated_models.is_empty() {
        "(none)".to_owned()
    } else {
        report.zero_rated_models.join("\n")
    };
    table.add_row(vec!["Zero-rated models".to_owned(), zero_rated]);
    let local = if report.local_models.is_empty() {
        "(none)".to_owned()
    } else {
        report.local_models.join("\n")
    };
    table.add_row(vec!["Local models".to_owned(), local]);
    let shadow = &report.shadow;
    table.add_row(vec![
        "Unpriced tokens".to_owned(),
        if shadow.tokens == 0 {
            "(none)".to_owned()
        } else {
            format!(
                "{} across {} models",
                group_thousands(shadow.tokens),
                shadow.models.len()
            )
        },
    ]);
    table.add_row(vec![
        "Shadow estimate".to_owned(),
        if shadow.estimate.is_zero() {
            "(none)".to_owned()
        } else {
            // doctor takes no --precise, so two places as elsewhere here.
            format!(
                "{} at reference rates (see [shadow] in pricing)",
                money(shadow.estimate, false)
            )
        },
    ]);
    // Only rendered inside WSL, where a second Claude Code install writes to
    // the Windows profile. Everywhere else the row would be permanent noise.
    if !report.unscanned_windows_roots.is_empty() {
        let mut value: Vec<String> = report
            .unscanned_windows_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        value.push(String::new());
        value
            .push("Windows-side Claude Code transcripts, not counted. To include them,".to_owned());
        value.push("add their .claude dirs to CLAUDE_CONFIG_DIR (comma-separated).".to_owned());
        table.add_row(vec!["Unscanned (WSL)".to_owned(), value.join("\n")]);
    }
    table.to_string()
}

/// Render the billing-blocks report: an optional active-block headline, one
/// row per block, and a totals row.
pub fn blocks(report: &crate::blocks::BlocksReport, tz: Tz, precise: bool) -> String {
    let active = report
        .blocks
        .iter()
        .find_map(|b| b.projection.as_ref().map(|p| (b, p)));
    let headline = match active {
        Some((b, p)) => format!(
            "Active block: {} so far, ~{} projected by {} ({:.0} tok/min).\n\n",
            money(b.totals.cost, precise),
            money(p.projected_cost, precise),
            b.end.with_timezone(&tz).format("%H:%M"),
            p.burn_tokens_per_min,
        ),
        None => String::new(),
    };

    let mut table = new_table([
        "Block (5h)",
        "Status",
        "Models",
        "Total Tokens",
        "Cost (USD)",
    ]);
    for b in &report.blocks {
        let window = format!(
            "{} – {}",
            b.start.with_timezone(&tz).format("%m-%d %H:%M"),
            b.end.with_timezone(&tz).format("%H:%M"),
        );
        let (status, cost) = match &b.projection {
            Some(p) => (
                format!("● active · {} left", human_duration(p.remaining)),
                format!(
                    "{} → ~{}",
                    money(b.totals.cost, precise),
                    money(p.projected_cost, precise)
                ),
            ),
            None => (format!("{BLOCK_HOURS}h"), money(b.totals.cost, precise)),
        };
        table.add_row(vec![
            window,
            status,
            b.models.join(", "),
            group_thousands(b.totals.total()),
            cost,
        ]);
    }
    table.add_row(vec![
        "Total".to_owned(),
        String::new(),
        String::new(),
        group_thousands(report.total.total()),
        money(report.total.cost, precise),
    ]);
    format!("{headline}{table}")
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

    /// A one-model cache report with the given write split and premium.
    fn ttl_report(write_5m: u64, write_1h: u64, premium: &str) -> crate::cache::CacheReport {
        let row = crate::cache::CacheEconomics {
            model: "claude-fable-5".to_owned(),
            totals: Totals {
                cache_write_5m: write_5m,
                cache_write_1h: write_1h,
                cache_read: 1_000,
                ..Totals::default()
            },
            hit_rate: None,
            actual_cost: rust_decimal::Decimal::ONE,
            counterfactual_cost: rust_decimal::Decimal::TWO,
            savings: rust_decimal::Decimal::ONE,
            leverage: Some(rust_decimal::Decimal::TWO),
            ttl_premium: premium.parse().unwrap(),
        };
        crate::cache::CacheReport {
            models: vec![row.clone()],
            total: crate::cache::CacheEconomics {
                model: "Total".to_owned(),
                ..row
            },
        }
    }

    /// The single write column becomes two, and the narrative reports the
    /// 1-hour share alongside what it cost.
    #[test]
    fn cache_table_splits_write_columns_and_reports_the_ttl_premium() {
        let rendered = cache(&ttl_report(1_000_000, 3_000_000, "22.5"), false);
        assert!(rendered.contains("Write 5m"), "{rendered}");
        assert!(rendered.contains("Write 1h"), "{rendered}");
        assert!(!rendered.contains("Cache Write"), "{rendered}");
        // Decimal's Display trims trailing zeros, matching `percent` and
        // `money` elsewhere in this module ("$0", not "$0.00").
        assert!(
            rendered.contains("75% of your cache writes used the 1-hour TTL"),
            "{rendered}"
        );
        assert!(rendered.contains("$22.5 more"), "{rendered}");
    }

    /// With no 1-hour writes there is no premium to report, so the sentence
    /// is omitted rather than printing a $0.00 line.
    #[test]
    fn cache_table_omits_the_ttl_sentence_without_1h_writes() {
        let rendered = cache(&ttl_report(1_000_000, 0, "0"), false);
        assert!(!rendered.contains("1-hour TTL"), "{rendered}");
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
            zero_rated_models: vec!["gpt-5.2-codex".into()],
            local_models: vec!["qwen3.6:27b".into()],
            shadow: crate::cost::ShadowDiagnostic {
                tokens: 391_208_320,
                models: vec!["gpt-5-codex".into()],
                estimate: "283.41".parse().unwrap(),
                mappings: [("gpt-5-codex".to_owned(), "gpt-5.4".to_owned())]
                    .into_iter()
                    .collect(),
            },
            unscanned_windows_roots: Vec::new(),
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
        assert!(rendered.contains("Zero-rated models"));
        assert!(rendered.contains("gpt-5.2-codex"));
        assert!(rendered.contains("Local models"));
        assert!(rendered.contains("qwen3.6:27b"));
        assert!(
            !rendered.contains("Unscanned (WSL)"),
            "WSL row must stay hidden when there is nothing to report:\n{rendered}"
        );

        // Same report with a Windows-side root: the row appears and names
        // the remedy, so the hint is actionable without consulting the docs.
        let with_hint = DoctorReport {
            unscanned_windows_roots: vec!["/mnt/c/Users/v/.claude/projects".into()],
            ..report
        };
        let rendered = doctor(&with_hint);
        assert!(rendered.contains("Unscanned (WSL)"));
        assert!(rendered.contains("/mnt/c/Users/v/.claude/projects"));
        assert!(rendered.contains("CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn blocks_renders_window_status_and_active_projection() {
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
                models: vec!["opus".into()],
                projection: Some(BlockProjection {
                    projected_cost: "6.00".parse().unwrap(),
                    projected_tokens: 600,
                    burn_tokens_per_min: 2.0,
                    remaining: chrono::Duration::minutes(160),
                }),
            }],
            total: totals,
        };
        let rendered = blocks(&report, chrono_tz::UTC, false);
        assert!(rendered.contains("Active block:"), "{rendered}");
        assert!(rendered.contains("07-04 12:00"), "{rendered}");
        assert!(rendered.contains("active"), "{rendered}");
        assert!(rendered.contains("$6.00"), "{rendered}");
        assert!(rendered.contains("Total"), "{rendered}");
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
