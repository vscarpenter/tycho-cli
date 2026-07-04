//! The end-to-end scan pipeline: discover → parse → dedupe.
//!
//! This is the seam the CLI calls. Filters that can skip whole files
//! (project) apply before parsing; per-event filters (model) apply before
//! deduplication. Files that cannot be opened are counted, never fatal.

use std::path::PathBuf;

use crate::dedupe::Deduper;
use crate::discover;
use crate::record::{self, ParseStats, UsageEvent};

/// Substring filters borrowed from the CLI arguments for the duration of
/// one scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventFilter<'a> {
    /// Keep only projects whose encoded directory name contains this.
    pub project: Option<&'a str>,
    /// Keep only events whose model id contains this.
    pub model: Option<&'a str>,
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
pub fn scan(roots: &[PathBuf], filter: EventFilter<'_>) -> ScanOutcome {
    let mut summary = ScanSummary::default();
    let mut deduper = Deduper::new();

    for file in discover::discover(roots) {
        if let Some(project) = filter.project
            && !file.project.contains(project)
        {
            continue;
        }
        let Ok(file_scan) = record::parse_file(&file.path) else {
            summary.files_unreadable += 1;
            continue;
        };
        summary.files_scanned += 1;
        summary.bytes_scanned += std::fs::metadata(&file.path).map(|m| m.len()).unwrap_or(0);
        summary.stats.merge(&file_scan.stats);
        for mut event in file_scan.events {
            if let Some(model) = filter.model
                && !event.model.contains(model)
            {
                continue;
            }
            event.project = file.project.clone();
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
}

/// Assemble the doctor report from a finished scan. Filters that were
/// applied to the scan apply to this report too.
pub fn doctor(roots: &[PathBuf], outcome: &ScanOutcome) -> DoctorReport {
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
            .map(|path| RootStatus {
                path: path.clone(),
                exists: path.is_dir(),
            })
            .collect(),
        summary: outcome.summary,
        date_span,
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const EVENT_TEMPLATE: &str = r#"{"type":"assistant","uuid":"UUID","timestamp":"TS","sessionId":"sess-1","requestId":"REQ","message":{"id":"MSG","model":"MODEL","usage":{"input_tokens":1,"output_tokens":OUT,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;

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
    fn dedupes_across_files_and_keeps_max_output() {
        let root = fixture_root();
        let outcome = scan(&[root.path().to_path_buf()], EventFilter::default());
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
        let outcome = scan(&[root.path().to_path_buf()], EventFilter::default());
        assert_eq!(outcome.summary.files_scanned, 3);
        assert_eq!(outcome.summary.files_unreadable, 0);
        assert_eq!(outcome.summary.stats.events, 4);
        assert!(outcome.summary.bytes_scanned > 0);
    }

    #[test]
    fn events_carry_the_project_they_were_found_under() {
        let root = fixture_root();
        let mut outcome = scan(&[root.path().to_path_buf()], EventFilter::default());
        outcome.events.sort_by(|a, b| a.project.cmp(&b.project));
        assert_eq!(outcome.events[0].project, "-Users-v-Projects-gsd");
        assert_eq!(outcome.events[1].project, "-Users-v-Projects-other");
    }

    #[test]
    fn project_filter_matches_encoded_directory_substring() {
        let root = fixture_root();
        let filter = EventFilter {
            project: Some("gsd"),
            ..Default::default()
        };
        let outcome = scan(&[root.path().to_path_buf()], filter);
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].model, "claude-opus-4-8");
        // Whole non-matching files are never parsed.
        assert_eq!(outcome.summary.files_scanned, 2);
    }

    #[test]
    fn model_filter_matches_model_substring() {
        let root = fixture_root();
        let filter = EventFilter {
            model: Some("sonnet"),
            ..Default::default()
        };
        let outcome = scan(&[root.path().to_path_buf()], filter);
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.events[0].usage.output, 42);
    }

    #[test]
    fn doctor_reports_roots_span_and_models() {
        let root = fixture_root();
        let missing = PathBuf::from("/definitely/not/here");
        let roots = vec![root.path().to_path_buf(), missing.clone()];
        let outcome = scan(&roots, EventFilter::default());
        let report = doctor(&roots, &outcome);

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
        assert_eq!(report.summary.duplicates_collapsed, 2);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_are_counted_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let root = fixture_root();
        let blocked = root.path().join("-Users-v-Projects-gsd/sess-1.jsonl");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let outcome = scan(&[root.path().to_path_buf()], EventFilter::default());
        assert_eq!(outcome.summary.files_unreadable, 1);
        assert_eq!(outcome.events.len(), 2);

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o644)).unwrap();
    }
}
