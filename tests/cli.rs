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

#[test]
fn monthly_json_buckets_the_whole_fixture_into_one_month() {
    let value = stdout_json(tycho().args(["monthly", "--json"]));
    let months = value["months"].as_array().unwrap();
    assert_eq!(months.len(), 1);
    assert_eq!(months[0]["month"], "2026-07");
    assert_eq!(months[0]["tokens"]["total"], 1_510);
}

#[test]
fn sessions_json_spans_subagents_and_sorts_by_recent_start() {
    let value = stdout_json(tycho().args(["sessions", "--json"]));
    assert_eq!(value["matching_sessions"], 2);
    let sessions = value["sessions"].as_array().unwrap();

    // sess-b started 2026-07-03, most recent first.
    assert_eq!(sessions[0]["session_id"], "sess-b");
    assert_eq!(sessions[0]["start"], "2026-07-03T04:30:00Z"); // subagent record
    assert_eq!(sessions[0]["duration_seconds"], 19_800);
    assert_eq!(
        sessions[0]["models"],
        serde_json::json!(["claude-opus-4-8"])
    );
    assert_eq!(sessions[0]["tokens"]["total"], 50);

    // sess-a spans two days; the duplicate collapsed to the record written
    // at 10:00:02, so the span starts there.
    assert_eq!(sessions[1]["session_id"], "sess-a");
    assert_eq!(sessions[1]["duration_seconds"], 86_398);
    assert_eq!(
        sessions[1]["models"],
        serde_json::json!(["claude-opus-4-8", "claude-sonnet-5"])
    );
    assert_eq!(sessions[1]["tokens"]["total"], 1_460);
}

#[test]
fn sessions_limit_keeps_grand_totals() {
    let value =
        stdout_json(tycho().args(["sessions", "--json", "--limit", "1", "--sort", "tokens"]));
    assert_eq!(value["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(value["sessions"][0]["session_id"], "sess-a"); // largest
    assert_eq!(value["matching_sessions"], 2);
    assert_eq!(value["totals"]["total"], 1_510);
}

#[test]
fn projects_json_rolls_up_with_session_counts() {
    let value = stdout_json(tycho().args(["projects", "--json"]));
    let projects = value["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0]["project"], "-Users-v-Projects-alpha");
    assert_eq!(projects[0]["sessions"], 1);
    assert_eq!(projects[0]["tokens"]["total"], 1_460);
    assert_eq!(projects[1]["project"], "-Users-v-Projects-beta");
    assert_eq!(projects[1]["last_activity"], "2026-07-03T10:00:00Z");
}

#[test]
fn models_json_rolls_up_largest_first() {
    let value = stdout_json(tycho().args(["models", "--json"]));
    let models = value["models"].as_array().unwrap();
    assert_eq!(models[0]["model"], "claude-opus-4-8");
    assert_eq!(models[0]["tokens"]["total"], 1_410);
    assert_eq!(models[1]["model"], "claude-sonnet-5");
    assert_eq!(models[1]["tokens"]["total"], 100);
}

#[test]
fn doctor_json_reports_health_counters() {
    let value = stdout_json(tycho().args(["doctor", "--json"]));
    assert_eq!(value["files"]["scanned"], 4);
    assert_eq!(value["lines"]["total"], 10);
    assert_eq!(value["lines"]["events"], 5);
    assert_eq!(value["lines"]["malformed"], 1);
    assert_eq!(value["lines"]["not_assistant"], 2);
    assert_eq!(value["lines"]["missing_usage"], 1);
    assert_eq!(value["lines"]["synthetic"], 1);
    assert_eq!(value["duplicates_collapsed"], 1);
    // The dedup survivor of msg_a1 is the 10:00:02 record.
    assert_eq!(value["date_span"]["first"], "2026-07-01T10:00:02Z");
    assert_eq!(value["date_span"]["last"], "2026-07-03T10:00:00Z");
    assert!(value["files"]["bytes"].as_u64().unwrap() > 0);
}

#[test]
fn daily_csv_emits_data_rows() {
    let assert = tycho().args(["daily", "--csv"]).assert().success();
    let rendered = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(rendered.starts_with("date,input,output"));
    assert!(rendered.contains("2026-07-01,100,200,50,10,1000,1360"));
}

#[test]
fn csv_on_models_is_a_usage_error() {
    tycho().args(["models", "--csv"]).assert().code(2);
}

fn cost_of(value: &serde_json::Value) -> f64 {
    value["totals"]["cost_usd"].as_f64().unwrap()
}

/// Hand-computed from the fixtures and the embedded pricing table:
/// calculate = (6412.5 + 303 + 303.25 + 116.25) / 1e6; auto swaps msg_a2's
/// calculated 303/1e6 for its recorded costUSD of 0.5.
#[test]
fn cost_modes_change_the_math() {
    let auto = stdout_json(tycho().args(["daily", "--json"]));
    assert!((cost_of(&auto) - 0.506_832).abs() < 1e-9, "auto: {auto}");

    let calc = stdout_json(tycho().args(["daily", "--json", "--mode", "calculate"]));
    assert!(
        (cost_of(&calc) - 0.007_135).abs() < 1e-9,
        "calculate: {calc}"
    );

    let display = stdout_json(tycho().args(["daily", "--json", "--mode", "display"]));
    assert!((cost_of(&display) - 0.5).abs() < 1e-9, "display: {display}");
}

#[test]
fn pricing_override_replaces_model_rates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pricing.toml");
    std::fs::write(
        &path,
        r#"
        [models."claude-opus-4-8"]
        input = 0.0
        output = 0.0
        cache_write_5m = 0.0
        cache_write_1h = 0.0
        cache_read = 0.0
        "#,
    )
    .unwrap();

    let value = stdout_json(tycho().args([
        "daily",
        "--json",
        "--mode",
        "calculate",
        "--pricing",
        path.to_str().unwrap(),
    ]));
    // Only the sonnet event still costs anything: 303 / 1e6.
    assert!((cost_of(&value) - 0.000_303).abs() < 1e-9, "{value}");
}

/// Fixture economics in calculate mode, hand-computed:
/// opus:   actual = 6832/1e6; counterfactual = (106+60+22+1014)*5/1e6 + 208*25/1e6 = 0.01121
/// sonnet: actual = 303/1e6;  counterfactual = (10+30+40)*2/1e6 + 20*10/1e6 = 0.00036
#[test]
fn cache_report_computes_counterfactual_savings() {
    let value = stdout_json(tycho().args(["cache", "--json", "--mode", "calculate"]));
    assert_eq!(value["command"], "cache");

    let opus = &value["models"][0];
    assert_eq!(opus["model"], "claude-opus-4-8");
    assert!((opus["hit_rate"].as_f64().unwrap() - 1014.0 / 1202.0).abs() < 1e-9);
    assert!((opus["counterfactual_cost_usd"].as_f64().unwrap() - 0.01121).abs() < 1e-9);

    let totals = &value["totals"];
    assert!((totals["actual_cost_usd"].as_f64().unwrap() - 0.007_135).abs() < 1e-9);
    assert!((totals["counterfactual_cost_usd"].as_f64().unwrap() - 0.01157).abs() < 1e-9);
    assert!((totals["savings_usd"].as_f64().unwrap() - 0.004_435).abs() < 1e-9);
    assert!((totals["leverage"].as_f64().unwrap() - 0.01157 / 0.007_135).abs() < 1e-9);
}

#[test]
fn cache_table_has_the_headline() {
    let assert = tycho()
        .args(["cache", "--mode", "calculate", "--precise"])
        .assert()
        .success();
    let rendered = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(rendered.contains("Caching saved you"), "{rendered}");
    assert!(rendered.contains("Hit Rate"), "{rendered}");
    assert!(rendered.contains("claude-opus-4-8"), "{rendered}");
}

#[test]
fn live_json_emits_a_snapshot() {
    // Piped stdout is non-TTY, so bare `live` also emits JSON; assert both.
    for args in [&["live", "--json"][..], &["live"][..]] {
        let value = stdout_json(tycho().args(args));
        assert_eq!(value["command"], "live");
        assert!(value["generated_at"].is_string());
        assert!(value["today"]["tokens"]["total"].is_number());
        assert_eq!(value["burn"]["per_minute"].as_array().unwrap().len(), 10);
        assert!(value["models"].is_array());
        assert!(value["sessions"].is_array());
    }
}

#[test]
fn blocks_json_groups_usage_into_windows() {
    let value = stdout_json(tycho().args(["blocks", "--json"]));
    assert_eq!(value["command"], "blocks");
    let blocks = value["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty());
    let b0 = &blocks[0];
    assert!(b0["start"].is_string());
    assert!(b0["end"].is_string());
    assert!(b0["tokens"]["total"].is_number());
    // Fixtures are all in the past, so no active block.
    assert_eq!(b0["active"], false);
    // Grand total matches the whole fixture corpus (see the daily test).
    assert_eq!(value["totals"]["total"], 1_510);
}

#[test]
fn blocks_csv_is_a_usage_error() {
    tycho().args(["blocks", "--csv"]).assert().code(2);
}

#[test]
fn invalid_pricing_file_is_a_runtime_error() {
    tycho()
        .args(["daily", "--pricing", "/definitely/not/a/file.toml"])
        .assert()
        .code(1);
}

/// Pi's agent directory, whose `sessions/` subtree the CLI discovers when
/// `PI_CODING_AGENT_DIR` points at it.
fn pi_agent_dir() -> String {
    format!("{}/tests/fixtures/pi", env!("CARGO_MANIFEST_DIR"))
}

/// A `tycho` invocation that sees ONLY the Pi fixture: `HOME` is redirected
/// to an empty temp directory so the machine's real Claude and Codex roots
/// cannot leak into the assertions.
fn tycho_pi(home: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("tycho").unwrap();
    cmd.env("HOME", home.path())
        .env("PI_CODING_AGENT_DIR", pi_agent_dir())
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .args(["--utc"]);
    cmd
}

#[test]
fn pi_sessions_are_discovered_and_reported_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let value = stdout_json(tycho_pi(&home).args(["models", "--json", "--provider", "pi"]));

    let models = value["models"].as_array().unwrap();
    assert_eq!(models.len(), 2, "one row per model, decoy records skipped");

    let bedrock = models
        .iter()
        .find(|m| m["model"] == "us.anthropic.claude-opus-4-6-v1")
        .unwrap();
    assert_eq!(bedrock["tokens"]["input"], 3);
    assert_eq!(bedrock["tokens"]["output"], 46);
    assert_eq!(bedrock["tokens"]["cache_read"], 11);
    // No TTL breakdown in Pi's data: the whole write is priced at 5m.
    assert_eq!(bedrock["tokens"]["cache_write_5m"], 7_491);
    assert_eq!(bedrock["tokens"]["cache_write_1h"], 0);

    let ollama = models
        .iter()
        .find(|m| m["model"] == "glm-5.3:cloud")
        .unwrap();
    assert_eq!(ollama["tokens"]["input"], 100);
    assert_eq!(ollama["tokens"]["cache_read"], 500);

    // 103 input + 66 output + 511 cache read + 7,531 cache write.
    assert_eq!(value["totals"]["total"], 8_211);
}

/// The point of reading `cwd` from the session record: Pi work lands under
/// the same project row as Claude Code work in the same repository, rather
/// than under Pi's own `--Users-v-Projects-alpha--` directory encoding.
#[test]
fn pi_events_take_their_project_from_the_session_cwd() {
    let home = tempfile::tempdir().unwrap();
    let value = stdout_json(tycho_pi(&home).args(["projects", "--json"]));
    let projects = value["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["project"], "-Users-v-Projects-alpha");
}

/// Pi's own `usage.cost.total` is the only truthful source for a
/// Bedrock-prefixed id: `longest_prefix_match` cannot resolve
/// `us.anthropic.claude-opus-4-6-v1` onto `claude-opus-4-6`, so calculate
/// mode reports $0 where auto mode reports what Pi actually billed.
#[test]
fn pi_recorded_cost_prices_a_model_the_table_cannot() {
    let home = tempfile::tempdir().unwrap();
    let auto = stdout_json(tycho_pi(&home).args(["daily", "--json"]));
    assert_eq!(auto["totals"]["cost_usd"], 0.04798375);

    let calculated = stdout_json(tycho_pi(&home).args(["daily", "--json", "--mode", "calculate"]));
    assert_eq!(calculated["totals"]["cost_usd"], 0.0);
}

/// `blocks` mirrors Claude's 5-hour reset, so it stays Claude-only unless
/// widened; a Pi-only tree therefore reports nothing until asked.
#[test]
fn blocks_ignores_pi_events_until_the_provider_is_widened() {
    let home = tempfile::tempdir().unwrap();
    let claude_only = stdout_json(tycho_pi(&home).args(["blocks", "--json"]));
    assert!(claude_only["blocks"].as_array().unwrap().is_empty());

    let widened = stdout_json(tycho_pi(&home).args(["blocks", "--json", "--provider", "pi"]));
    assert_eq!(widened["blocks"].as_array().unwrap().len(), 1);
}
