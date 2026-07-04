//! End-to-end CLI tests against the committed synthetic fixtures.
//!
//! The fixture tree covers the spec's required cases: happy path, duplicate
//! streaming records, malformed lines, missing usage, unknown record types,
//! `costUSD` present, a `<synthetic>` record, an empty file, TTL breakdown
//! present and absent, and a subagent transcript merging into its session's
//! project. Expected numbers are hand-computed in each assertion.

// Everything in this file is test code; the crate-wide unwrap ban targets
// library/binary code (clippy's allow-unwrap-in-tests exempts #[test]
// functions but not test helpers).
#![allow(clippy::unwrap_used)]

use assert_cmd::Command;

fn fixtures_dir() -> String {
    format!("{}/tests/fixtures/projects", env!("CARGO_MANIFEST_DIR"))
}

/// A `tycho` invocation pinned to the fixture tree and UTC for determinism.
fn tycho() -> Command {
    let mut cmd = Command::cargo_bin("tycho").unwrap();
    cmd.args(["--dir", &fixtures_dir(), "--utc"]);
    cmd
}

fn stdout_json(cmd: &mut Command) -> serde_json::Value {
    let assert = cmd.assert().success();
    serde_json::from_slice(&assert.get_output().stdout).unwrap()
}

#[test]
fn daily_json_reports_deduplicated_totals_end_to_end() {
    let value = stdout_json(tycho().args(["daily", "--json"]));
    assert_eq!(value["command"], "daily");
    assert_eq!(value["timezone"], "UTC");

    let days = value["days"].as_array().unwrap();
    assert_eq!(days.len(), 3);

    // 07-01: the streaming duplicate collapsed to the max-output record.
    assert_eq!(days[0]["date"], "2026-07-01");
    assert_eq!(days[0]["tokens"]["output"], 200);
    assert_eq!(days[0]["tokens"]["cache_write_5m"], 50);
    assert_eq!(days[0]["tokens"]["cache_write_1h"], 10);
    assert_eq!(days[0]["tokens"]["total"], 1_360);

    // 07-02: no TTL breakdown, so the aggregate count lands in the 5m bucket.
    assert_eq!(days[1]["tokens"]["cache_write_5m"], 30);
    assert_eq!(days[1]["tokens"]["cache_write_1h"], 0);

    // 07-03: session and subagent transcripts merge.
    assert_eq!(days[2]["tokens"]["total"], 50);

    assert_eq!(value["totals"]["input"], 116);
    assert_eq!(value["totals"]["output"], 228);
    assert_eq!(value["totals"]["cache_read"], 1_054);
    assert_eq!(value["totals"]["total"], 1_510);
}

#[test]
fn bare_invocation_is_daily() {
    let bare = tycho().arg("--json").assert().success();
    let explicit = tycho().args(["daily", "--json"]).assert().success();
    assert_eq!(bare.get_output().stdout, explicit.get_output().stdout);
}

#[test]
fn project_filter_limits_scope() {
    let value = stdout_json(tycho().args(["daily", "--json", "--project", "beta"]));
    let days = value["days"].as_array().unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0]["date"], "2026-07-03");
    assert_eq!(value["totals"]["total"], 50);
}

#[test]
fn model_filter_limits_scope() {
    let value = stdout_json(tycho().args(["daily", "--json", "--model", "sonnet"]));
    assert_eq!(value["totals"]["input"], 10);
    assert_eq!(value["totals"]["total"], 100);
}

#[test]
fn since_and_until_bound_the_report() {
    let value = stdout_json(tycho().args([
        "daily",
        "--json",
        "--since",
        "2026-07-02",
        "--until",
        "2026-07-02",
    ]));
    let days = value["days"].as_array().unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0]["date"], "2026-07-02");
}

#[test]
fn tz_and_utc_together_is_a_usage_error_with_exit_2() {
    Command::cargo_bin("tycho")
        .unwrap()
        .args(["--tz", "America/Chicago", "--utc", "daily"])
        .assert()
        .code(2);
}

#[test]
fn table_output_has_formatted_counts_and_a_totals_row() {
    let assert = tycho().arg("daily").assert().success();
    let rendered = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(rendered.contains("2026-07-01"));
    assert!(rendered.contains("1,360"), "day total:\n{rendered}");
    assert!(rendered.contains("Total"), "totals row:\n{rendered}");
    assert!(rendered.contains("1,510"), "grand total:\n{rendered}");
}
