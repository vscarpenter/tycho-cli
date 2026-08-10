//! The end-to-end scan pipeline: discover → parse → dedupe.
//!
//! This is the seam the CLI calls. `--project` is path-authoritative for
//! Claude roots, so it skips whole files there before parsing. Codex and
//! External roots resolve project per event instead — from Codex's `cwd`
//! metadata when a context record supplied one, else from the file's own
//! directory position — so `--project` applies there per event, alongside
//! `--model` and `--provider`, before deduplication. Files that cannot be
//! opened are counted, never fatal.

use std::path::PathBuf;

use crate::dedupe::Deduper;
use crate::discover::{self, Provider, SearchRoot, TranscriptFile};
use crate::record::{self, ParseStats, UsageEvent};

/// Substring filters borrowed from the CLI arguments for the duration of
/// one scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventFilter<'a> {
    /// Keep only projects whose encoded directory name contains this.
    pub project: Option<&'a str>,
    /// Keep only events whose model id contains this.
    pub model: Option<&'a str>,
    /// Keep only events produced by this provider.
    pub provider: Option<Provider>,
}

/// Everything `doctor` and humans need to know about a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Transcript files parsed.
    pub files_scanned: u64,
    /// Files that could not be opened or read.
    pub files_unreadable: u64,
    /// Duplicate streaming records collapsed (ADR 0002).
    pub duplicates_collapsed: u64,
    /// Total size of the parsed transcript files, in bytes.
    pub bytes_scanned: u64,
    /// Line-level parse counters merged across files.
    pub stats: ParseStats,
}

/// Deduplicated events plus the scan's bookkeeping.
#[derive(Debug, Default)]
pub struct ScanOutcome {
    /// One event per API message, ready for aggregation.
    pub events: Vec<UsageEvent>,
    /// Scan statistics.
    pub summary: ScanSummary,
}

/// Scan all roots, honoring `filter`, and return deduplicated events.
///
/// Files parse in parallel (rayon); the merge stays sequential and in
/// discovery order, so results are deterministic regardless of thread
/// scheduling. `filter.project` prefilters whole files here, but only for
/// Claude roots: their encoded project directory is path-authoritative, so a
/// non-matching Claude file need not be opened at all. Codex and External
/// files always parse instead — their project may come from in-file metadata
/// (Codex's `cwd`) or, when no record supplies one, from the file's own
/// directory position — and get the same `project` substring check per event
/// in [`scan_files`].
pub fn scan(roots: &[SearchRoot], filter: EventFilter<'_>) -> ScanOutcome {
    let files: Vec<_> = discover::discover(roots)
        .into_iter()
        .filter(|file| {
            file.provider != Provider::Claude
                || filter
                    .project
                    .is_none_or(|project| file.project.contains(project))
        })
        .collect();
    scan_files(files, filter)
}

/// Parse and deduplicate an already-discovered file list. The `project`,
/// `model`, and `provider` filters are all applied per event here (no
/// whole-file skipping — that only happens in [`scan`], and only for Claude
/// roots). This is the seam `tycho live` uses so it can read file mtimes
/// from the same discovery pass (see `crate::tui::compute_snapshot`).
pub fn scan_files(files: Vec<TranscriptFile>, filter: EventFilter<'_>) -> ScanOutcome {
    use rayon::prelude::*;

    let parsed: Vec<_> = files
        .into_par_iter()
        .map(|file| {
            let bytes = std::fs::metadata(&file.path).map(|m| m.len()).unwrap_or(0);
            let scan = record::parse_file(&file.path, file.provider);
            (file, bytes, scan)
        })
        .collect();

    let mut summary = ScanSummary::default();
    let mut deduper = Deduper::new();
    for (file, bytes, scan) in parsed {
        let Ok(file_scan) = scan else {
            summary.files_unreadable += 1;
            continue;
        };
        summary.files_scanned += 1;
        summary.bytes_scanned += bytes;
        summary.stats.merge(&file_scan.stats);
        for mut event in file_scan.events {
            let project: &str = if event.project.is_empty() {
                &file.project
            } else {
                &event.project
            };
            if let Some(wanted) = filter.project
                && !project.contains(wanted)
            {
                continue;
            }
            if let Some(model) = filter.model
                && !event.model.contains(model)
            {
                continue;
            }
            if let Some(provider) = filter.provider
                && event.provider != provider
            {
                continue;
            }
            if event.project.is_empty() {
                event.project = file.project.clone();
            }
            deduper.insert(event);
        }
    }

    summary.duplicates_collapsed = deduper.collapsed();
    ScanOutcome {
        events: deduper.into_events().collect(),
        summary,
    }
}

/// One search root and whether it exists on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootStatus {
    /// The root path as configured.
    pub path: PathBuf,
    /// Whether the directory exists.
    pub exists: bool,
}

/// Data-health report for the `doctor` command, assembled from a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct DoctorReport {
    /// Every configured search root and its existence.
    pub roots: Vec<RootStatus>,
    /// Scan bookkeeping (files, bytes, line counters, duplicates).
    pub summary: ScanSummary,
    /// Earliest and latest event timestamps observed, if any events exist.
    pub date_span: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
    /// Distinct model ids observed, sorted.
    pub models: Vec<String>,
    /// Observed models with no pricing entry (priced at zero), sorted.
    pub unpriced_models: Vec<String>,
    /// Observed models whose matched pricing entry has every rate at zero,
    /// sorted.
    pub zero_rated_models: Vec<String>,
    /// Observed Ollama-style `name:tag` models with no pricing entry,
    /// sorted.
    pub local_models: Vec<String>,
    /// Tokens carrying no real rates, and what they would cost at the
    /// pricing table's `[shadow]` reference rates. Computed by the cost
    /// engine and passed in, like the three model lists above. Diagnostic
    /// only — no report total includes it.
    pub shadow: crate::cost::ShadowDiagnostic,
    /// Windows-side Claude Code roots visible from WSL that are not being
    /// scanned. Empty everywhere except inside WSL.
    pub unscanned_windows_roots: Vec<PathBuf>,
}

/// Assemble the doctor report from a finished scan. Filters that were
/// applied to the scan apply to this report too: under `--project`, the
/// summary counters (`files_scanned` and friends) cover Claude files whose
/// path-encoded project matched, plus *all* Codex and External files, since
/// those providers are only filtered per event, after parsing.
/// `unpriced_models`, `zero_rated_models`, and `local_models` all come from
/// the cost engine so this module stays pricing-agnostic.
pub fn doctor(
    roots: &[SearchRoot],
    outcome: &ScanOutcome,
    unpriced_models: Vec<String>,
    zero_rated_models: Vec<String>,
    local_models: Vec<String>,
    shadow: crate::cost::ShadowDiagnostic,
    unscanned_windows_roots: Vec<PathBuf>,
) -> DoctorReport {
    let date_span = outcome
        .events
        .iter()
        .map(|event| event.timestamp)
        .fold(None, |span, ts| match span {
            None => Some((ts, ts)),
            Some((first, last)) => Some((first.min(ts), last.max(ts))),
        });
    // BTreeSet gives distinct + sorted in one collect.
    let models: Vec<String> = outcome
        .events
        .iter()
        .map(|event| event.model.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    DoctorReport {
        roots: roots
            .iter()
            .map(|root| RootStatus {
                path: root.path.clone(),
                exists: root.path.is_dir(),
            })
            .collect(),
        summary: outcome.summary,
        date_span,
        models,
        unpriced_models,
        zero_rated_models,
        local_models,
        shadow,
        unscanned_windows_roots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const EVENT_TEMPLATE: &str = r#"{"type":"assistant","uuid":"UUID","timestamp":"TS","sessionId":"sess-1","requestId":"REQ","message":{"id":"MSG","model":"MODEL","usage":{"input_tokens":1,"output_tokens":OUT,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;

    /// Wrap a fixture directory as a Claude-layout search root.
    fn claude_root(dir: &tempfile::TempDir) -> SearchRoot {
        SearchRoot {
            path: dir.path().to_path_buf(),
            provider: discover::Provider::Claude,
        }
    }

    /// Wrap a fixture directory as a Codex-layout search root.
    fn codex_root(dir: &tempfile::TempDir) -> SearchRoot {
        SearchRoot {
            path: dir.path().to_path_buf(),
            provider: discover::Provider::Codex,
        }
    }

    /// One Codex rollout file whose `cwd` is `alpha`, deliberately never
    /// matching the Claude fixtures' `gsd`/`other` projects, so tests can
    /// tell "always parsed" apart from "coincidentally matched".
    fn codex_fixture_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("rollout-codex-sess.jsonl"),
            [
                r#"{"type":"session_meta","timestamp":"2026-07-08T01:40:26.000Z","payload":{"id":"codex-sess","session_id":"codex-sess","cwd":"/Users/v/Projects/alpha"}}"#,
                r#"{"type":"turn_context","timestamp":"2026-07-08T01:40:27.000Z","payload":{"model":"gpt-5.5","cwd":"/Users/v/Projects/alpha"}}"#,
                r#"{"type":"event_msg","timestamp":"2026-07-08T01:40:35.000Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":10}}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        dir
    }

    fn line(msg: &str, req: &str, model: &str, out: u64) -> String {
        EVENT_TEMPLATE
            .replace("UUID", &format!("u-{msg}-{out}"))
            .replace("TS", "2026-07-02T10:00:00Z")
            .replace("REQ", req)
            .replace("MSG", msg)
            .replace("MODEL", model)
            .replace("OUT", &out.to_string())
    }

    /// Two projects; the duplicate message appears in both a session file
    /// and a subagent file to prove dedup spans files.
    fn fixture_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let gsd = dir.path().join("-Users-v-Projects-gsd");
        fs::create_dir_all(gsd.join("sess-1/subagents")).unwrap();
        fs::write(
            gsd.join("sess-1.jsonl"),
            [
                line("msg_1", "req_1", "claude-opus-4-8", 100),
                line("msg_1", "req_1", "claude-opus-4-8", 500),
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            gsd.join("sess-1/subagents/agent-a1.jsonl"),
            line("msg_1", "req_1", "claude-opus-4-8", 250),
        )
        .unwrap();

        let other = dir.path().join("-Users-v-Projects-other");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("sess-2.jsonl"),
            line("msg_2", "req_2", "claude-sonnet-5", 42),
        )
        .unwrap();
        dir
    }

    #[test]
    fn scan_files_parses_a_prediscovered_list() {
        let root = fixture_root();
        let files = crate::discover::discover(&[claude_root(&root)]);
        let outcome = scan_files(files, EventFilter::default());
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.summary.files_scanned, 3);
        assert_eq!(outcome.summary.duplicates_collapsed, 2);
    }

    #[test]
    fn dedupes_across_files_and_keeps_max_output() {
        let root = fixture_root();
        let outcome = scan(&[claude_root(&root)], EventFilter::default());
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.summary.duplicates_collapsed, 2);
        let survivor = outcome
            .events
            .iter()
            .find(|e| e.model == "claude-opus-4-8")
            .unwrap();
        assert_eq!(survivor.usage.output, 500);
    }

    #[test]
    fn counts_files_scanned() {
        let root = fixture_root();
        let outcome = scan(&[claude_root(&root)], EventFilter::default());
        assert_eq!(outcome.summary.files_scanned, 3);
        assert_eq!(outcome.summary.files_unreadable, 0);
        assert_eq!(outcome.summary.stats.events, 4);
        assert!(outcome.summary.bytes_scanned > 0);
    }

    #[test]
    fn events_carry_the_project_they_were_found_under() {
        let root = fixture_root();
        let mut outcome = scan(&[claude_root(&root)], EventFilter::default());
        outcome.events.sort_by(|a, b| a.project.cmp(&b.project));
        assert_eq!(outcome.events[0].project, "-Users-v-Projects-gsd");
        assert_eq!(outcome.events[1].project, "-Users-v-Projects-other");
    }

    #[test]
    fn project_filter_skips_nonmatching_claude_files_but_parses_codex() {
        let claude = fixture_root();
        let codex = codex_fixture_root();
        let filter = EventFilter {
            project: Some("gsd"),
            ..Default::default()
        };
        let outcome = scan(&[claude_root(&claude), codex_root(&codex)], filter);
        // Claude's "other" project is skipped before parsing (path-authoritative);
        // Codex's file is always parsed even though its cwd-derived project
        // ("alpha") does not match "gsd" either.
        assert_eq!(outcome.summary.files_scanned, 3);
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].model, "claude-opus-4-8");
    }

    #[test]
    fn provider_filter_selects_events() {
        let claude = fixture_root();
        let codex = codex_fixture_root();
        let filter = EventFilter {
            provider: Some(discover::Provider::Codex),
            ..Default::default()
        };
        let outcome = scan(&[claude_root(&claude), codex_root(&codex)], filter);
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].provider, discover::Provider::Codex);
    }

    #[test]
    fn model_filter_matches_model_substring() {
        let root = fixture_root();
        let filter = EventFilter {
            model: Some("sonnet"),
            ..Default::default()
        };
        let outcome = scan(&[claude_root(&root)], filter);
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].usage.output, 42);
    }

    #[test]
    fn doctor_reports_roots_span_and_models() {
        let root = fixture_root();
        let missing = PathBuf::from("/definitely/not/here");
        let roots = vec![
            claude_root(&root),
            SearchRoot {
                path: missing.clone(),
                provider: discover::Provider::Claude,
            },
        ];
        let outcome = scan(&roots, EventFilter::default());
        let report = doctor(
            &roots,
            &outcome,
            vec!["mystery-model".into()],
            vec!["gpt-5.2-codex".into()],
            vec!["qwen3.6:27b".into()],
            crate::cost::ShadowDiagnostic::default(),
            Vec::new(),
        );

        assert_eq!(report.roots.len(), 2);
        assert!(report.roots[0].exists);
        assert_eq!(
            report.roots[1],
            RootStatus {
                path: missing,
                exists: false
            }
        );

        let (first, last) = report.date_span.unwrap();
        assert_eq!(first, last); // all fixture events share one timestamp
        assert_eq!(report.models, ["claude-opus-4-8", "claude-sonnet-5"]);
        assert_eq!(report.unpriced_models, ["mystery-model"]);
        assert_eq!(report.zero_rated_models, ["gpt-5.2-codex"]);
        assert_eq!(report.local_models, ["qwen3.6:27b"]);
        assert_eq!(report.summary.duplicates_collapsed, 2);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_are_counted_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let root = fixture_root();
        let blocked = root.path().join("-Users-v-Projects-gsd/sess-1.jsonl");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let outcome = scan(&[claude_root(&root)], EventFilter::default());
        assert_eq!(outcome.summary.files_unreadable, 1);
        assert_eq!(outcome.events.len(), 2);

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o644)).unwrap();
    }
}
