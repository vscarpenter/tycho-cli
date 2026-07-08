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

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::discover::Provider;

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

/// One model/API usage event, validated and ready for aggregation.
/// Everything downstream of parsing trusts these fields.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    /// When the record was written (UTC; reports bucket in a target zone).
    pub timestamp: DateTime<Utc>,
    /// Session UUID; subagent transcripts carry the parent session's id.
    pub session_id: Option<String>,
    /// Encoded project directory the transcript was found under. Empty at
    /// parse time; the scan pipeline fills it from the file's location.
    pub project: String,
    /// Model id, e.g. `claude-opus-4-8`.
    pub model: String,
    /// Token counts for this message.
    pub usage: TokenUsage,
    /// Pre-computed cost from older Claude Code versions, if present.
    pub cost_usd: Option<f64>,
    /// Cost in USD, stamped by the cost engine after parsing (zero until
    /// then; see `crate::cost::Coster::apply`).
    pub cost: rust_decimal::Decimal,
    /// Identity used to collapse duplicate streaming records.
    pub dedup_key: DedupKey,
    /// The provider of the format that parsed this record (stamped at
    /// parse time, not sniffed again downstream).
    pub provider: Provider,
}

/// Why a line did not produce a [`UsageEvent`]. Skips are data, not errors:
/// `doctor` reports their counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The line was not valid JSON or had the wrong shape.
    Malformed,
    /// A valid record that does not carry usage metadata.
    NotAssistant,
    /// A usage-bearing record with no usage payload.
    MissingUsage,
    /// A usage-bearing record with no parseable timestamp.
    MissingTimestamp,
    /// A usage-bearing record with no model id.
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
    /// Valid records that do not carry usage metadata.
    pub not_assistant: u64,
    /// Usage-bearing records lacking usage.
    pub missing_usage: u64,
    /// Usage-bearing records lacking a timestamp.
    pub missing_timestamp: u64,
    /// Usage-bearing records lacking a model.
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

/// The permissive envelope for one JSONL line, of any supported transcript or
/// response-log shape. Every field is optional and unknown fields are ignored.
/// Deliberately absent: any field that could hold message content.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRecord {
    #[serde(rename = "type")]
    record_type: Option<String>,
    uuid: Option<String>,
    timestamp: Option<RawTimestamp>,
    created: Option<RawTimestamp>,
    #[serde(alias = "created_at")]
    created_at: Option<RawTimestamp>,
    id: Option<String>,
    model: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(alias = "request_id")]
    request_id: Option<String>,
    is_api_error_message: Option<bool>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    message: Option<RawMessage>,
    usage: Option<RawUsage>,
    payload: Option<RawCodexPayload>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<RawCacheCreation>,
    input_tokens_details: Option<RawTokenDetails>,
    prompt_tokens_details: Option<RawTokenDetails>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Deserialize)]
struct RawTokenDetails {
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct RawCodexPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    id: Option<String>,
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    #[serde(rename = "turnId", alias = "turn_id")]
    turn_id: Option<String>,
    info: Option<RawCodexInfo>,
}

#[derive(Deserialize)]
struct RawCodexInfo {
    last_token_usage: Option<RawUsage>,
}

/// A permissive-but-bounded timestamp: RFC3339 strings or integer epoch
/// seconds within [2000-01-01, 2100-01-01). Anything else deserializes to
/// `None` so the record skips as `MissingTimestamp` instead of inventing
/// absurd dates (observed failure: millisecond epochs landing in year
/// ~58486). Non-scalar JSON still fails the whole record, as before.
struct RawTimestamp(Option<DateTime<Utc>>);

const EPOCH_MIN: i64 = 946_684_800; // 2000-01-01T00:00:00Z
const EPOCH_MAX: i64 = 4_102_444_800; // 2100-01-01T00:00:00Z

fn bounded_epoch(value: i64) -> Option<DateTime<Utc>> {
    (EPOCH_MIN..EPOCH_MAX)
        .contains(&value)
        .then(|| Utc.timestamp_opt(value, 0).single())
        .flatten()
}

impl RawTimestamp {
    fn to_utc(&self) -> Option<DateTime<Utc>> {
        self.0
    }
}

impl<'de> serde::Deserialize<'de> for RawTimestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = RawTimestamp;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an RFC3339 string or epoch seconds")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<RawTimestamp, E> {
                Ok(RawTimestamp(v.parse::<DateTime<Utc>>().ok()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<RawTimestamp, E> {
                Ok(RawTimestamp(bounded_epoch(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<RawTimestamp, E> {
                Ok(RawTimestamp(i64::try_from(v).ok().and_then(bounded_epoch)))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<RawTimestamp, E> {
                Ok(RawTimestamp(None))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl RawUsage {
    /// Without a TTL breakdown, the aggregate cache-write count is priced
    /// at the 5-minute rate (documented in docs/SCHEMA.md).
    fn claude_token_usage(&self) -> TokenUsage {
        let cache_creation_input_tokens = self.cache_creation_input_tokens.unwrap_or(0);
        let (cache_write_5m, cache_write_1h) = match &self.cache_creation {
            Some(split) => (
                split.ephemeral_5m_input_tokens,
                split.ephemeral_1h_input_tokens,
            ),
            None => (cache_creation_input_tokens, 0),
        };
        TokenUsage {
            input: self.input_tokens.unwrap_or(0),
            output: self.output_tokens.unwrap_or(0),
            cache_write_5m,
            cache_write_1h,
            cache_read: self.cache_read_input_tokens.unwrap_or(0),
        }
    }

    /// OpenAI usage reports cached input as a subset of total input. Keep the
    /// report's total token math intact by splitting total input into uncached
    /// input plus cache reads.
    fn openai_token_usage(&self) -> Option<TokenUsage> {
        let total_input = self.input_tokens.or(self.prompt_tokens)?;
        let cached = self
            .cached_input_tokens
            .or_else(|| {
                self.input_tokens_details
                    .as_ref()
                    .and_then(|details| details.cached_tokens)
            })
            .or_else(|| {
                self.prompt_tokens_details
                    .as_ref()
                    .and_then(|details| details.cached_tokens)
            })
            .unwrap_or(0)
            .min(total_input);
        Some(TokenUsage {
            input: total_input - cached,
            output: self.output_tokens.or(self.completion_tokens).unwrap_or(0),
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: cached,
        })
    }
}

/// Parse one line with no cross-line state, sniffing all supported formats
/// (the `External` rules). Codex token_count lines need [`parse_file`]'s
/// stateful context and will skip here with `MissingModel`-style reasons.
pub fn parse_line(line: &str) -> Result<UsageEvent, SkipReason> {
    LineParser {
        provider: Provider::External,
        ..LineParser::default()
    }
    .parse_line(line)
}

struct LineParser {
    provider: Provider,
    fallback_session_id: Option<String>,
    codex_session_id: Option<String>,
    codex_project: Option<String>,
    codex_model: Option<String>,
    codex_turn_id: Option<String>,
    codex_usage_index: u64,
}

impl Default for LineParser {
    fn default() -> Self {
        Self {
            provider: Provider::External,
            fallback_session_id: None,
            codex_session_id: None,
            codex_project: None,
            codex_model: None,
            codex_turn_id: None,
            codex_usage_index: 0,
        }
    }
}

impl LineParser {
    fn new(path: &Path, provider: Provider) -> Self {
        Self {
            provider,
            fallback_session_id: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned()),
            ..Self::default()
        }
    }

    fn parse_line(&mut self, line: &str) -> Result<UsageEvent, SkipReason> {
        let raw: RawRecord = serde_json::from_str(line).map_err(|_| SkipReason::Malformed)?;
        match self.provider {
            Provider::Claude => {
                if raw.record_type.as_deref() == Some("assistant") {
                    parse_claude_record(raw)
                } else {
                    Err(SkipReason::NotAssistant)
                }
            }
            Provider::Codex => {
                self.capture_codex_context(&raw);
                self.parse_codex_token_count(&raw)?
                    .ok_or(SkipReason::NotAssistant)
            }
            Provider::External => {
                if raw.record_type.as_deref() == Some("assistant") {
                    return parse_claude_record(raw);
                }
                self.capture_codex_context(&raw);
                if let Some(event) = self.parse_codex_token_count(&raw)? {
                    return Ok(event);
                }
                parse_openai_record(&raw)?.ok_or(SkipReason::NotAssistant)
            }
        }
    }

    fn capture_codex_context(&mut self, raw: &RawRecord) {
        let Some(payload) = raw.payload.as_ref() else {
            return;
        };
        match raw.record_type.as_deref() {
            Some("session_meta") => {
                self.codex_session_id = payload
                    .session_id
                    .clone()
                    .or_else(|| payload.id.clone())
                    .or_else(|| self.fallback_session_id.clone());
                if let Some(cwd) = &payload.cwd {
                    self.codex_project = Some(cwd.clone());
                }
            }
            Some("turn_context") => {
                if let Some(model) = &payload.model {
                    self.codex_model = Some(model.clone());
                }
                if let Some(cwd) = &payload.cwd {
                    self.codex_project = Some(cwd.clone());
                }
                self.codex_turn_id = payload.turn_id.clone();
            }
            _ => {}
        }
    }

    fn parse_codex_token_count(
        &mut self,
        raw: &RawRecord,
    ) -> Result<Option<UsageEvent>, SkipReason> {
        if raw.record_type.as_deref() != Some("event_msg") {
            return Ok(None);
        }
        let Some(payload) = raw.payload.as_ref() else {
            return Ok(None);
        };
        if payload.payload_type.as_deref() != Some("token_count") {
            return Ok(None);
        }
        let usage = payload
            .info
            .as_ref()
            .and_then(|info| info.last_token_usage.as_ref())
            .and_then(RawUsage::openai_token_usage)
            .ok_or(SkipReason::MissingUsage)?;
        let timestamp = raw
            .timestamp
            .as_ref()
            .and_then(RawTimestamp::to_utc)
            .ok_or(SkipReason::MissingTimestamp)?;
        let model = self
            .codex_model
            .clone()
            .or_else(|| payload.model.clone())
            .ok_or(SkipReason::MissingModel)?;
        self.codex_usage_index += 1;
        let session_id = self
            .codex_session_id
            .clone()
            .or_else(|| self.fallback_session_id.clone());
        let session_label = session_id.as_deref().unwrap_or("(unknown)").to_owned();
        let turn_label = self
            .codex_turn_id
            .as_deref()
            .unwrap_or("(unknown)")
            .to_owned();
        let dedup_key = DedupKey::Uuid(format!(
            "codex:{session_label}:{turn_label}:{}",
            self.codex_usage_index
        ));
        Ok(Some(UsageEvent {
            timestamp,
            session_id,
            project: self.codex_project.clone().unwrap_or_default(),
            model,
            usage,
            cost_usd: None,
            cost: rust_decimal::Decimal::ZERO,
            dedup_key,
            provider: Provider::Codex,
        }))
    }
}

/// Consumes `raw` by value: the caller has already confirmed `record_type`
/// is `"assistant"`, so this moves `message`/`model`/ids/`session_id`
/// straight into the event instead of cloning them.
fn parse_claude_record(raw: RawRecord) -> Result<UsageEvent, SkipReason> {
    let message = raw.message.ok_or(SkipReason::MissingUsage)?;
    if raw.is_api_error_message == Some(true) || message.model.as_deref() == Some("<synthetic>") {
        return Err(SkipReason::SyntheticApiError);
    }
    let usage = message
        .usage
        .as_ref()
        .ok_or(SkipReason::MissingUsage)?
        .claude_token_usage();
    let timestamp = raw
        .timestamp
        .as_ref()
        .and_then(RawTimestamp::to_utc)
        .ok_or(SkipReason::MissingTimestamp)?;
    let model = message.model.ok_or(SkipReason::MissingModel)?;
    let dedup_key = match (message.id, raw.request_id) {
        (Some(message_id), Some(request_id)) => DedupKey::MessageRequest(message_id, request_id),
        _ => DedupKey::Uuid(raw.uuid.ok_or(SkipReason::MissingIdentity)?),
    };
    Ok(UsageEvent {
        timestamp,
        session_id: raw.session_id,
        project: String::new(),
        model,
        usage,
        cost_usd: raw.cost_usd,
        cost: rust_decimal::Decimal::ZERO,
        dedup_key,
        provider: Provider::Claude,
    })
}

fn parse_openai_record(raw: &RawRecord) -> Result<Option<UsageEvent>, SkipReason> {
    let Some(raw_usage) = raw.usage.as_ref() else {
        return Ok(None);
    };
    let usage = raw_usage
        .openai_token_usage()
        .ok_or(SkipReason::MissingUsage)?;
    let timestamp = raw
        .timestamp
        .as_ref()
        .and_then(RawTimestamp::to_utc)
        .or_else(|| raw.created_at.as_ref().and_then(RawTimestamp::to_utc))
        .or_else(|| raw.created.as_ref().and_then(RawTimestamp::to_utc))
        .ok_or(SkipReason::MissingTimestamp)?;
    let model = raw.model.clone().ok_or(SkipReason::MissingModel)?;
    let id = raw.id.clone().ok_or(SkipReason::MissingIdentity)?;
    Ok(Some(UsageEvent {
        timestamp,
        session_id: Some(id.clone()),
        project: String::new(),
        model,
        usage,
        cost_usd: raw.cost_usd,
        cost: rust_decimal::Decimal::ZERO,
        dedup_key: DedupKey::Uuid(id),
        provider: Provider::External,
    }))
}

/// Stream a transcript file line by line, dispatching by the root's
/// provider (see [`LineParser::parse_line`]). Only I/O problems (open/read
/// failures) are `Err`; content problems are counted in
/// [`FileScan::stats`]. Lines that are not valid UTF-8 are decoded lossily
/// rather than aborting the file.
pub fn parse_file(path: &Path, provider: Provider) -> io::Result<FileScan> {
    let mut reader = io::BufReader::new(std::fs::File::open(path)?);
    let mut scan = FileScan::default();
    let mut parser = LineParser::new(path, provider);
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
        match parser.parse_line(line) {
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
    fn parses_openai_responses_usage_and_cached_input() {
        let line = r#"{"id":"resp_1","object":"response","created_at":1783476000,"model":"gpt-5.4","usage":{"input_tokens":1000,"output_tokens":50,"total_tokens":1050,"input_tokens_details":{"cached_tokens":400},"output_tokens_details":{"reasoning_tokens":10}}}"#;
        let event = parse_line(line).unwrap();
        assert_eq!(event.model, "gpt-5.4");
        assert_eq!(event.session_id.as_deref(), Some("resp_1"));
        assert_eq!(event.timestamp.to_rfc3339(), "2026-07-08T02:00:00+00:00");
        assert_eq!(
            event.usage,
            TokenUsage {
                input: 600,
                output: 50,
                cache_write_5m: 0,
                cache_write_1h: 0,
                cache_read: 400,
            }
        );
        assert_eq!(event.dedup_key, DedupKey::Uuid("resp_1".into()));
        assert_eq!(event.provider, Provider::External);
    }

    #[test]
    fn parses_openai_chat_completion_usage() {
        let line = r#"{"id":"chatcmpl_1","object":"chat.completion","created":1783479600,"model":"chat-latest","usage":{"prompt_tokens":1000,"completion_tokens":100,"total_tokens":1100,"prompt_tokens_details":{"cached_tokens":100},"completion_tokens_details":{"reasoning_tokens":0}}}"#;
        let event = parse_line(line).unwrap();
        assert_eq!(event.model, "chat-latest");
        assert_eq!(event.usage.input, 900);
        assert_eq!(event.usage.cache_read, 100);
        assert_eq!(event.usage.output, 100);
        assert_eq!(event.provider, Provider::External);
    }

    #[test]
    fn millisecond_epochs_are_rejected_not_far_future() {
        let line = r#"{"id":"resp_ms","created_at":1783476000000,"model":"gpt-5.4","usage":{"input_tokens":10,"output_tokens":1}}"#;
        assert_eq!(parse_line(line).unwrap_err(), SkipReason::MissingTimestamp);
    }

    #[test]
    fn integer_string_timestamps_are_not_epochs() {
        // At one point "1234" parsed as 1970-01-01T00:20:34Z; it must skip instead.
        let line = r#"{"type":"assistant","uuid":"u-1","timestamp":"1234","requestId":"req_A","message":{"id":"msg_A","model":"m","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        assert_eq!(parse_line(line).unwrap_err(), SkipReason::MissingTimestamp);
    }

    #[test]
    fn plausible_integer_epochs_still_parse() {
        let line = r#"{"id":"resp_ok","created_at":1783476000,"model":"gpt-5.4","usage":{"input_tokens":10,"output_tokens":1}}"#;
        assert_eq!(
            parse_line(line).unwrap().timestamp.to_rfc3339(),
            "2026-07-08T02:00:00+00:00"
        );
    }

    #[test]
    fn float_timestamps_are_rejected() {
        let line = r#"{"id":"resp_f","created_at":1783476000.5,"model":"gpt-5.4","usage":{"input_tokens":10,"output_tokens":1}}"#;
        assert_eq!(parse_line(line).unwrap_err(), SkipReason::MissingTimestamp);
    }

    #[test]
    fn parse_file_uses_codex_context_for_token_count_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-codex-sess.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"session_meta","timestamp":"2026-07-08T01:40:26.000Z","payload":{"id":"codex-sess","session_id":"codex-sess","cwd":"/Users/v/Projects/openai","source":"vscode","originator":"Codex Desktop","model_provider":"openai"}}"#,
                r#"{"type":"turn_context","timestamp":"2026-07-08T01:40:27.000Z","payload":{"model":"gpt-5.5","cwd":"/Users/v/Projects/openai","turn_id":"turn-1"}}"#,
                r#"{"type":"event_msg","timestamp":"2026-07-08T01:40:35.000Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":1050},"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":1050},"model_context_window":258400}}}"#,
                r#"{"type":"event_msg","timestamp":"2026-07-08T01:40:42.000Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200,"cached_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":220},"total_token_usage":{"input_tokens":1200,"cached_input_tokens":400,"output_tokens":70,"reasoning_output_tokens":10,"total_tokens":1270},"model_context_window":258400}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let scan = parse_file(&path, Provider::Codex).unwrap();
        assert_eq!(scan.stats.lines, 4);
        assert_eq!(scan.stats.events, 2);
        assert_eq!(scan.stats.not_assistant, 2);
        assert_eq!(scan.events[0].session_id.as_deref(), Some("codex-sess"));
        assert_eq!(scan.events[0].project, "/Users/v/Projects/openai");
        assert_eq!(scan.events[0].model, "gpt-5.5");
        assert_eq!(scan.events[0].usage.input, 600);
        assert_eq!(scan.events[0].usage.cache_read, 400);
        assert_eq!(scan.events[0].provider, Provider::Codex);
        assert_eq!(scan.events[1].usage.input, 200);
        assert_ne!(scan.events[0].dedup_key, scan.events[1].dedup_key);
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

        let scan = parse_file(&path, Provider::Claude).unwrap();
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
        let scan = parse_file(&path, Provider::Claude).unwrap();
        assert!(scan.events.is_empty());
        assert_eq!(scan.stats, ParseStats::default());
    }

    #[test]
    fn dual_key_session_id_records_still_parse() {
        // SCHEMA.md documents session_id as a snake_case duplicate of sessionId;
        // records carrying both must not be rejected as duplicate serde fields.
        let line = r#"{"type":"assistant","uuid":"u-2","sessionId":"sess-1","session_id":"sess-1","timestamp":"2026-07-08T01:00:00Z","requestId":"req_B","message":{"id":"msg_B","model":"m","usage":{"input_tokens":2,"output_tokens":2}}}"#;
        let event = parse_line(line).unwrap();
        assert_eq!(event.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn usage_null_chunks_are_other_record_types_not_missing_usage() {
        let line = r#"{"id":"chatcmpl_c1","object":"chat.completion.chunk","created":1783479600,"model":"chat-latest","usage":null}"#;
        assert_eq!(parse_line(line).unwrap_err(), SkipReason::NotAssistant);
    }

    #[test]
    fn nested_response_wrappers_are_not_parsed() {
        // Undocumented shape (SCHEMA.md documents no embedded response object);
        // parsing it double-counted Codex sessions that also log token_count.
        let line = r#"{"type":"response_item","timestamp":"2026-07-08T01:00:00Z","payload":{"response":{"id":"resp_a","object":"response","model":"gpt-5.5","usage":{"input_tokens":1000,"output_tokens":50}}}}"#;
        assert_eq!(parse_line(line).unwrap_err(), SkipReason::NotAssistant);
    }

    #[test]
    fn claude_roots_never_parse_foreign_usage_lines() {
        // A foreign log line with a top-level usage object must not fabricate
        // an event when the file lives under a Claude root.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        std::fs::write(&path, r#"{"id":"log_1","event":"llm_call","created_at":1783476000,"model":"gpt-5.4","usage":{"prompt_tokens":123,"completion_tokens":4}}"#).unwrap();
        let scan = parse_file(&path, Provider::Claude).unwrap();
        assert_eq!(scan.stats.events, 0);
        assert_eq!(scan.stats.not_assistant, 1);
        // The same line under an External root IS an event (explicit --dir opt-in).
        let scan = parse_file(&path, Provider::External).unwrap();
        assert_eq!(scan.stats.events, 1);
    }

    #[test]
    fn events_carry_the_format_provider() {
        let claude = r#"{"type":"assistant","uuid":"u-1","timestamp":"2026-07-08T01:00:00Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-8","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        assert_eq!(parse_line(claude).unwrap().provider, Provider::Claude);
        let openai = r#"{"id":"resp_1","created_at":1783476000,"model":"gpt-5.4","usage":{"input_tokens":10,"output_tokens":1}}"#;
        assert_eq!(parse_line(openai).unwrap().provider, Provider::External);
    }
}
