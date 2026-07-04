//! Parsing of transcript lines into usage events.
//!
//! Privacy is enforced structurally here: the serde types capture only the
//! metadata envelope (ids, timestamp, model, usage). There is no field that
//! could hold message content, so content can never enter the program.
//! See `docs/adr/0001-permissive-metadata-envelope.md`.
//!
//! Malformed input is normal input: every line either becomes a
//! [`UsageEvent`] or a counted [`SkipReason`] — never a fatal error.

use std::io::{self, BufRead};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Deduplicated token counts for one API message, with cache writes split
/// by TTL (see `docs/SCHEMA.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Uncached input tokens.
    pub input: u64,
    /// Output tokens (may be a mid-stream undercount; see SCHEMA.md).
    pub output: u64,
    /// Cache writes with a 5-minute TTL.
    pub cache_write_5m: u64,
    /// Cache writes with a 1-hour TTL.
    pub cache_write_1h: u64,
    /// Tokens read from cache.
    pub cache_read: u64,
}

/// Identity of a record for deduplication (see ADR 0002).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DedupKey {
    /// `(message.id, requestId)` — the primary identity.
    MessageRequest(String, String),
    /// Fallback when either id is missing.
    Uuid(String),
}

/// One assistant API message, validated and ready for aggregation.
/// Everything downstream of parsing trusts these fields.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    /// When the record was written (UTC; reports bucket in a target zone).
    pub timestamp: DateTime<Utc>,
    /// Session UUID; subagent transcripts carry the parent session's id.
    pub session_id: Option<String>,
    /// Model id, e.g. `claude-opus-4-8`.
    pub model: String,
    /// Token counts for this message.
    pub usage: TokenUsage,
    /// Pre-computed cost from older Claude Code versions, if present.
    pub cost_usd: Option<f64>,
    /// Identity used to collapse duplicate streaming records.
    pub dedup_key: DedupKey,
}

/// Why a line did not produce a [`UsageEvent`]. Skips are data, not errors:
/// `doctor` reports their counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The line was not valid JSON or had the wrong shape.
    Malformed,
    /// A valid record of a type other than `assistant` (15+ types exist).
    NotAssistant,
    /// An assistant record with no `message.usage`.
    MissingUsage,
    /// An assistant record with no parseable timestamp.
    MissingTimestamp,
    /// An assistant record with no `message.model`.
    MissingModel,
    /// No `message.id`/`requestId` pair and no `uuid` to deduplicate by.
    MissingIdentity,
    /// An API-error placeholder (`model: "<synthetic>"`); zero usage.
    SyntheticApiError,
}

/// Counters for one scan, aggregated across files for `doctor`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseStats {
    /// Non-empty lines seen.
    pub lines: u64,
    /// Lines that became usage events.
    pub events: u64,
    /// Skips by reason.
    pub malformed: u64,
    /// Valid records of non-assistant types.
    pub not_assistant: u64,
    /// Assistant records lacking usage.
    pub missing_usage: u64,
    /// Assistant records lacking a timestamp.
    pub missing_timestamp: u64,
    /// Assistant records lacking a model.
    pub missing_model: u64,
    /// Assistant records lacking any dedup identity.
    pub missing_identity: u64,
    /// Synthetic API-error placeholders.
    pub synthetic: u64,
}

impl ParseStats {
    /// Record one skipped line.
    pub fn count_skip(&mut self, reason: SkipReason) {
        match reason {
            SkipReason::Malformed => self.malformed += 1,
            SkipReason::NotAssistant => self.not_assistant += 1,
            SkipReason::MissingUsage => self.missing_usage += 1,
            SkipReason::MissingTimestamp => self.missing_timestamp += 1,
            SkipReason::MissingModel => self.missing_model += 1,
            SkipReason::MissingIdentity => self.missing_identity += 1,
            SkipReason::SyntheticApiError => self.synthetic += 1,
        }
    }

    /// Merge another file's counters into this one.
    pub fn merge(&mut self, other: &ParseStats) {
        self.lines += other.lines;
        self.events += other.events;
        self.malformed += other.malformed;
        self.not_assistant += other.not_assistant;
        self.missing_usage += other.missing_usage;
        self.missing_timestamp += other.missing_timestamp;
        self.missing_model += other.missing_model;
        self.missing_identity += other.missing_identity;
        self.synthetic += other.synthetic;
    }
}

/// Everything one transcript file yielded.
#[derive(Debug, Default)]
pub struct FileScan {
    /// Usage events, not yet deduplicated.
    pub events: Vec<UsageEvent>,
    /// Skip counters for this file.
    pub stats: ParseStats,
}

/// The permissive envelope for one JSONL line, of any record type. Every
/// field is optional and unknown fields are ignored. Deliberately absent:
/// any field that could hold message content.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRecord {
    #[serde(rename = "type")]
    record_type: Option<String>,
    uuid: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    session_id: Option<String>,
    request_id: Option<String>,
    is_api_error_message: Option<bool>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<RawCacheCreation>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

impl RawUsage {
    /// Without a TTL breakdown, the aggregate cache-write count is priced
    /// at the 5-minute rate (documented in docs/SCHEMA.md).
    fn into_token_usage(self) -> TokenUsage {
        let (cache_write_5m, cache_write_1h) = match self.cache_creation {
            Some(split) => (
                split.ephemeral_5m_input_tokens,
                split.ephemeral_1h_input_tokens,
            ),
            None => (self.cache_creation_input_tokens, 0),
        };
        TokenUsage {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_write_5m,
            cache_write_1h,
            cache_read: self.cache_read_input_tokens,
        }
    }
}

/// Parse one transcript line. `Err` means "skip for this reason", never a
/// fatal condition.
pub fn parse_line(line: &str) -> Result<UsageEvent, SkipReason> {
    let raw: RawRecord = serde_json::from_str(line).map_err(|_| SkipReason::Malformed)?;
    if raw.record_type.as_deref() != Some("assistant") {
        return Err(SkipReason::NotAssistant);
    }
    let message = raw.message.ok_or(SkipReason::MissingUsage)?;
    if raw.is_api_error_message == Some(true) || message.model.as_deref() == Some("<synthetic>") {
        return Err(SkipReason::SyntheticApiError);
    }
    let usage = message.usage.ok_or(SkipReason::MissingUsage)?;
    let timestamp = raw.timestamp.ok_or(SkipReason::MissingTimestamp)?;
    let model = message.model.ok_or(SkipReason::MissingModel)?;
    let dedup_key = match (message.id, raw.request_id) {
        (Some(message_id), Some(request_id)) => DedupKey::MessageRequest(message_id, request_id),
        _ => DedupKey::Uuid(raw.uuid.ok_or(SkipReason::MissingIdentity)?),
    };
    Ok(UsageEvent {
        timestamp,
        session_id: raw.session_id,
        model,
        usage: usage.into_token_usage(),
        cost_usd: raw.cost_usd,
        dedup_key,
    })
}

/// Stream a transcript file line by line. Only I/O problems (open/read
/// failures) are `Err`; content problems are counted in
/// [`FileScan::stats`]. Lines that are not valid UTF-8 are decoded lossily
/// rather than aborting the file.
pub fn parse_file(path: &Path) -> io::Result<FileScan> {
    let mut reader = io::BufReader::new(std::fs::File::open(path)?);
    let mut scan = FileScan::default();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            return Ok(scan);
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        scan.stats.lines += 1;
        match parse_line(line) {
            Ok(event) => {
                scan.stats.events += 1;
                scan.events.push(event);
            }
            Err(reason) => scan.stats.count_skip(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic assistant record (synthetic values, real shape from
    /// docs/SCHEMA.md) with content fields present, as in real data.
    const FULL_RECORD: &str = r#"{"type":"assistant","uuid":"u-1","parentUuid":"u-0","timestamp":"2026-07-02T23:54:44.905Z","sessionId":"sess-1","requestId":"req_A","cwd":"/tmp/p","gitBranch":"main","version":"2.1.201","isSidechain":false,"message":{"id":"msg_A","model":"claude-opus-4-8","role":"assistant","content":[{"type":"text","text":"SECRET-DO-NOT-PARSE"}],"usage":{"input_tokens":2,"output_tokens":5301,"cache_creation_input_tokens":5521,"cache_read_input_tokens":196377,"service_tier":"standard","cache_creation":{"ephemeral_5m_input_tokens":4000,"ephemeral_1h_input_tokens":1521},"inference_geo":"us","iterations":null,"speed":null}}}"#;

    #[test]
    fn parses_a_realistic_assistant_record() {
        let event = parse_line(FULL_RECORD).unwrap();
        assert_eq!(event.model, "claude-opus-4-8");
        assert_eq!(event.session_id.as_deref(), Some("sess-1"));
        assert_eq!(
            event.timestamp.to_rfc3339(),
            "2026-07-02T23:54:44.905+00:00"
        );
        assert_eq!(
            event.usage,
            TokenUsage {
                input: 2,
                output: 5301,
                cache_write_5m: 4000,
                cache_write_1h: 1521,
                cache_read: 196377,
            }
        );
        assert_eq!(
            event.dedup_key,
            DedupKey::MessageRequest("msg_A".into(), "req_A".into())
        );
        assert_eq!(event.cost_usd, None);
    }

    #[test]
    fn falls_back_to_5m_bucket_when_ttl_breakdown_is_absent() {
        let line = r#"{"type":"assistant","uuid":"u-1","timestamp":"2026-07-02T00:00:00Z","requestId":"req_A","message":{"id":"msg_A","model":"m","usage":{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":4}}}"#;
        let event = parse_line(line).unwrap();
        assert_eq!(event.usage.cache_write_5m, 300);
        assert_eq!(event.usage.cache_write_1h, 0);
    }

    #[test]
    fn skips_user_records_and_every_other_known_type() {
        for record_type in ["user", "attachment", "file-history-snapshot", "system"] {
            let line = format!(r#"{{"type":"{record_type}","uuid":"u-1"}}"#);
            assert_eq!(parse_line(&line), Err(SkipReason::NotAssistant));
        }
    }

    #[test]
    fn skips_unknown_future_record_types_silently() {
        let line = r#"{"type":"holographic-context","someNewField":42}"#;
        assert_eq!(parse_line(line), Err(SkipReason::NotAssistant));
    }

    #[test]
    fn malformed_line_is_a_skip_not_a_panic() {
        assert_eq!(
            parse_line("not json at {{{ all"),
            Err(SkipReason::Malformed)
        );
        assert_eq!(parse_line(r#"[1,2,3]"#), Err(SkipReason::Malformed));
    }

    #[test]
    fn assistant_record_without_usage_is_counted_missing_usage() {
        let line = r#"{"type":"assistant","uuid":"u-1","timestamp":"2026-07-02T00:00:00Z","requestId":"req_A","message":{"id":"msg_A","model":"m"}}"#;
        assert_eq!(parse_line(line), Err(SkipReason::MissingUsage));
    }

    #[test]
    fn synthetic_api_error_records_are_skipped_and_counted() {
        let line = r#"{"type":"assistant","uuid":"u-1","timestamp":"2026-06-07T20:32:17.715Z","requestId":"req_B","isApiErrorMessage":true,"message":{"id":"515227e8-e06c-44ff-ad60-466a2f28116b","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"service_tier":null,"cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0},"inference_geo":null}}}"#;
        assert_eq!(parse_line(line), Err(SkipReason::SyntheticApiError));
    }

    #[test]
    fn dedup_key_falls_back_to_uuid_when_request_id_is_missing() {
        let line = r#"{"type":"assistant","uuid":"u-7","timestamp":"2026-07-02T00:00:00Z","message":{"id":"msg_A","model":"m","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        let event = parse_line(line).unwrap();
        assert_eq!(event.dedup_key, DedupKey::Uuid("u-7".into()));
    }

    #[test]
    fn cost_usd_is_captured_when_present() {
        let line = r#"{"type":"assistant","uuid":"u-1","timestamp":"2026-07-02T00:00:00Z","requestId":"req_A","costUSD":0.42,"message":{"id":"msg_A","model":"m","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        assert_eq!(parse_line(line).unwrap().cost_usd, Some(0.42));
    }

    #[test]
    fn missing_timestamp_is_its_own_skip_reason() {
        let line = r#"{"type":"assistant","uuid":"u-1","requestId":"req_A","message":{"id":"msg_A","model":"m","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#;
        assert_eq!(parse_line(line), Err(SkipReason::MissingTimestamp));
    }

    #[test]
    fn parse_file_streams_mixed_content_and_counts_every_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.jsonl");
        let mut lines: Vec<String> = vec![
            FULL_RECORD.to_string(),
            r#"{"type":"user","uuid":"u-2"}"#.to_string(),
            "garbage %%% line".to_string(),
        ];
        // An invalid-UTF-8 line must not abort the file.
        std::fs::write(
            &path,
            [
                lines.join("\n").into_bytes(),
                b"\n\xFF\xFE not utf8\n".to_vec(),
            ]
            .concat(),
        )
        .unwrap();
        lines.clear();

        let scan = parse_file(&path).unwrap();
        assert_eq!(scan.events.len(), 1);
        assert_eq!(scan.stats.lines, 4);
        assert_eq!(scan.stats.events, 1);
        assert_eq!(scan.stats.not_assistant, 1);
        assert_eq!(scan.stats.malformed, 2);
    }

    #[test]
    fn parse_file_handles_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let scan = parse_file(&path).unwrap();
        assert!(scan.events.is_empty());
        assert_eq!(scan.stats, ParseStats::default());
    }
}
