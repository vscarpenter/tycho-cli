# Provider-Tagged Parsing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve all 20 verified review findings by assigning provider identity per search root and fixing the parsing, dedup, project, filter, and pricing defects listed in `docs/superpowers/specs/2026-07-08-provider-tagged-parsing-design.md`.

**Architecture:** A `Provider` enum (`Claude`, `Codex`, `External`) is set per search root at discovery; `parse_file` dispatches parsers by root provider (content sniffing survives only under `External`, i.e. `--dir`). Events are stamped with the provider of the *format that parsed them*. Projects share one encoding (Claude's dash scheme). Pricing gains boundary-aware lookup and doctor visibility for zero-rated/local models.

**Tech Stack:** Rust (edition 2024), serde/serde_json, chrono, clap, rayon, rust_decimal. Tests via `cargo test`.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"` (Cargo.toml) — do not change.
- Privacy invariant: no deserialization field may ever hold message content (`src/lib.rs` doc).
- Parse failures are per-line skip-and-count, never fatal (`SkipReason`).
- Report JSON: fields are only ever added, never renamed/removed (README).
- TDD: write the failing test, see it fail, implement, see it pass, commit.
- Commits: Conventional Commits `type(scope): subject`, author `Vinny Carpenter <vscarpenter@gmail.com>`, final trailer line exactly `Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8`, NO `Co-Authored-By`/`Generated with` lines.
- After each task: `cargo test` must be fully green (existing tests updated within the task that breaks them).
- `cargo fmt` before every commit.

---

### Task 1: `Provider` enum, `SearchRoot`, provider-aware discovery, empty-`CODEX_HOME` fix

**Files:**
- Modify: `src/discover.rs`
- Modify: `src/main.rs` (`resolve_roots`, `run_live` caller if it builds roots)
- Modify: `src/scan.rs` (only the `discover::discover(...)` call signature + `TranscriptFile` uses; full scan logic changes come in Task 8)
- Modify: `src/tui/mod.rs` (only to keep compiling: `discover(...)` call sites)
- Test: `src/discover.rs` `mod tests`

**Interfaces (later tasks rely on these exact names):**
- `pub enum Provider { Claude, Codex, External }` — derives `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`. Lives in `src/discover.rs`, re-exported from `src/lib.rs` if other modules import via `crate::discover::Provider`.
- `pub struct SearchRoot { pub path: PathBuf, pub provider: Provider }` (derives `Debug, Clone, PartialEq, Eq`).
- `pub fn default_roots(home: &Path, claude_config_dir: Option<&str>, codex_home: Option<&str>) -> Vec<SearchRoot>`
- `pub struct TranscriptFile { pub project: String, pub path: PathBuf, pub provider: Provider }`
- `pub fn discover(roots: &[SearchRoot]) -> Vec<TranscriptFile>`
- Codex-root files get `project == "(codex)"`.

- [ ] **Step 1: Write the failing tests** (replace/extend existing `default_roots_*` tests; add codex placeholder + empty-env tests)

```rust
#[test]
fn default_roots_tag_providers() {
    let roots = default_roots(Path::new("/Users/v"), None, None);
    assert_eq!(
        roots,
        vec![
            SearchRoot { path: PathBuf::from("/Users/v/.claude/projects"), provider: Provider::Claude },
            SearchRoot { path: PathBuf::from("/Users/v/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"), provider: Provider::Claude },
            SearchRoot { path: PathBuf::from("/Users/v/.codex/sessions"), provider: Provider::Codex },
        ]
    );
}

#[test]
fn empty_codex_home_falls_back_to_home_codex() {
    // A set-but-empty CODEX_HOME must not yield a relative "sessions" root.
    for value in ["", "   "] {
        let roots = default_roots(Path::new("/Users/v"), None, Some(value));
        assert!(roots.iter().any(|r| r.path == PathBuf::from("/Users/v/.codex/sessions")));
    }
}

#[test]
fn codex_root_files_get_placeholder_project() {
    let dir = tempfile::tempdir().unwrap();
    let day = dir.path().join("2026/07/08");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("rollout-x.jsonl"), "{}\n").unwrap();
    let found = discover(&[SearchRoot { path: dir.path().to_path_buf(), provider: Provider::Codex }]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].project, "(codex)");
    assert_eq!(found[0].provider, Provider::Codex);
}
```

Also mechanically update the existing tests in this file to build `SearchRoot`s with `Provider::Claude` (fixture trees) and assert on `SearchRoot` values in the two `default_roots_*` tests (config-dir variant keeps its current path expectations plus providers Claude/Claude/Claude/Codex).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli discover::` — Expected: compile errors (`SearchRoot` not found), which counts as the failing state for a type-introduction step.

- [ ] **Step 3: Implement**

In `src/discover.rs`:

```rust
/// Which product wrote the files under a search root. Assigned per root at
/// discovery time; `External` (from `--dir`) is the only provider whose
/// files are format-sniffed line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provider {
    Claude,
    Codex,
    External,
}

/// A search root plus the provider that owns its layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
    pub path: PathBuf,
    pub provider: Provider,
}
```

`default_roots`: build Claude roots as today, then:

```rust
    let mut roots: Vec<SearchRoot> = /* claude paths */
        .into_iter()
        .map(|path| SearchRoot { path, provider: Provider::Claude })
        .collect();
    roots.push(SearchRoot {
        path: home.join("Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects"),
        provider: Provider::Claude,
    });
    let codex_base = codex_home
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    roots.push(SearchRoot { path: codex_base.join("sessions"), provider: Provider::Codex });
    roots
```

`TranscriptFile` gains `pub provider: Provider` (keep derive list; `Provider` already derives `Ord` so sorting still works — keep `project` as the first field so sort order stays project-then-path).

`discover(roots: &[SearchRoot])`: walk `root.path`, set `provider: root.provider`, and:

```rust
/// The first directory component below the root is the encoded project
/// name for Claude-layout roots. Codex roots are date-sharded
/// (`sessions/<yyyy>/<mm>/<dd>/`), so their files get the `(codex)`
/// placeholder; the real project comes from record metadata during the scan.
fn project_name(root: &Path, provider: Provider, file: &Path) -> String {
    if provider == Provider::Codex {
        return "(codex)".to_owned();
    }
    /* existing body unchanged */
}
```

In `src/main.rs` `resolve_roots` (returns `anyhow::Result<Vec<SearchRoot>>`):

```rust
    if !cli.global.dirs.is_empty() {
        return Ok(cli
            .global
            .dirs
            .iter()
            .cloned()
            .map(|path| discover::SearchRoot { path, provider: discover::Provider::External })
            .collect());
    }
```

Update `scan::scan`/`scan::doctor` signatures' `roots: &[PathBuf]` to `&[SearchRoot]` only as far as needed to compile (pass `root.path` where paths are printed); same for `src/tui/mod.rs` call sites. Do not change filter logic yet.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: all green (fix any straggler call sites the compiler flags).

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add -A src docs && git commit -m "feat(discover): tag search roots and files with a Provider

Provider enum (Claude/Codex/External) assigned per root; --dir roots are
External. Codex-root files get the (codex) placeholder project instead of
a date component, and a set-but-empty CODEX_HOME now falls back to
~/.codex instead of producing a relative sessions root.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 2: Bounded timestamp newtype (`RawTimestamp`)

**Files:**
- Modify: `src/record.rs` (replace the untagged `RawTimestamp` enum, `unix_seconds`, and `to_utc` uses)
- Test: `src/record.rs` `mod tests`

**Interfaces:**
- `struct RawTimestamp(Option<DateTime<Utc>>)` with a hand-written `Deserialize` impl and `fn to_utc(&self) -> Option<DateTime<Utc>>`.
- Accepted forms: RFC3339-style strings (whatever `str::parse::<DateTime<Utc>>()` accepts), integer epoch **seconds** in `[946_684_800, 4_102_444_800)` (2000-01-01 to 2100-01-01). Everything else (floats, out-of-range ints, non-datetime strings) → `RawTimestamp(None)`. Non-scalar JSON (object/array/bool) → deserialize error (whole record `Malformed`, as at HEAD).

- [ ] **Step 1: Write the failing tests**

```rust
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
```

(These use the current sniffing `parse_line`; they keep passing after Task 5 because stateless `parse_line` stays External.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli record::tests::millisecond` — Expected: FAIL (event parses with a year-58486 date instead of skipping).

- [ ] **Step 3: Implement**

Replace the enum + `unix_seconds` with:

```rust
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
```

`to_utc` call sites keep working (`.as_ref().and_then(RawTimestamp::to_utc)`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: all green, including the existing epoch tests (`parses_openai_responses_usage_and_cached_input` still expects `2026-07-08T02:00:00+00:00`).

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add src/record.rs && git commit -m "fix(record): bound timestamps to plausible epochs, RFC3339-only strings

Custom RawTimestamp deserializer replaces the untagged enum: integer
epoch seconds accepted only in [2000-01-01, 2100-01-01), floats and
integer-like strings rejected to MissingTimestamp. Kills the year-58486
millisecond-epoch dates, the \"1234\"->1970 regression, and the per-line
String allocation of the untagged variant.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 3: Delete speculative surface; usage-gated OpenAI candidate; dual-key alias fix

**Files:**
- Modify: `src/record.rs`
- Modify: `docs/SCHEMA.md` (remove the nested-response caveat implications; done fully in Task 12)
- Test: `src/record.rs` `mod tests`

**Interfaces:**
- `RawRecord` loses: `conversation_id`, `thread_id`, `object`, `response`; its `session_id` field keeps `rename = "sessionId"` but **no snake_case alias**.
- `RawCodexPayload` loses `response`. `RawOpenAiResponse`, `OpenAiCandidate`, `openai_candidate`, `looks_like_openai_object` are deleted.
- New signature: `fn parse_openai_record(raw: &RawRecord) -> Result<Option<UsageEvent>, SkipReason>` gated ONLY on `raw.usage.is_some()`; reads `raw.id`, `raw.model`, `raw.usage`, `raw.created`, `raw.created_at`, `raw.timestamp` directly. (Session/identity fallbacks move into `LineParser` in Task 5; in this task keep the current id-based session/dedup behavior so tests stay green.)

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli record::tests::dual_key record::tests::usage_null record::tests::nested_response` — Expected: all three FAIL (Malformed / MissingUsage / a parsed event, respectively).

- [ ] **Step 3: Implement**

- Remove `alias = "session_id"` from `RawRecord.session_id` (keep `rename = "sessionId"`).
- Delete fields/structs/functions listed under Interfaces. `RawRecord` keeps: `record_type`, `uuid`, `timestamp`, `created`, `created_at` (with its `alias = "created_at"`), `id`, `model`, `session_id`, `request_id`, `is_api_error_message`, `cost_usd`, `message`, `usage`, `payload`.
- Rewrite the OpenAI branch:

```rust
fn parse_openai_record(raw: &RawRecord) -> Result<Option<UsageEvent>, SkipReason> {
    let Some(raw_usage) = raw.usage.as_ref() else {
        return Ok(None);
    };
    let usage = raw_usage.openai_token_usage().ok_or(SkipReason::MissingUsage)?;
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
    }))
}
```

- Delete the now-dead test `parses_openai_chat_completion_usage`'s reliance on `object` (it still parses — usage present — keep the test as-is; only assertions about behavior stay valid).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: green. The existing `parses_openai_responses_usage_and_cached_input` and chat-completion tests must still pass (they carry `usage`).

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add src/record.rs && git commit -m "fix(record): drop dual-key alias and speculative OpenAI surface

Removing alias=session_id restores parsing of real Claude records that
carry both sessionId and session_id (serde rejects duplicate-mapped keys;
~5% of events in affected projects were counted malformed). Deletes
conversation_id/thread_id, nested response wrappers, RawOpenAiResponse,
OpenAiCandidate and looks_like_openai_object per the SCHEMA.md
documented-fields rule; OpenAI detection now requires a usage object, so
chunk lines with usage:null count as other record types and embedded
responses can no longer double-count Codex sessions.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 4: Provider-dispatched parsing (`parse_file`), format-stamped events

**Files:**
- Modify: `src/record.rs` (`LineParser`, `parse_file`, `parse_line`, `UsageEvent`)
- Modify: `src/scan.rs` (`parse_file` call), `src/dedupe.rs` tests, any test constructing `UsageEvent`
- Test: `src/record.rs` `mod tests`

**Interfaces:**
- `UsageEvent` gains `pub provider: Provider` (import `crate::discover::Provider`). Stamped by the FORMAT that parsed the record: Claude assistant → `Provider::Claude`, Codex token_count → `Provider::Codex`, standalone OpenAI → `Provider::External`.
- `pub fn parse_file(path: &Path, provider: Provider) -> io::Result<FileScan>`.
- `LineParser` gains `provider: Provider`; `LineParser::new(path, provider)`.
- Dispatch: `Claude` root → Claude parser only; `Codex` root → codex context + token_count only; `External` → assistant-typed lines to the Claude parser, codex envelope types to the codex parser, else OpenAI.
- Claude parsing consumes the record by value again: `fn parse_claude_record(raw: RawRecord) -> Result<UsageEvent, SkipReason>` (moves strings; no clones). Free function `pub fn parse_line(line: &str)` keeps existing behavior by delegating to a default `External` parser (doc comment updated).

- [ ] **Step 1: Write the failing tests**

```rust
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
```

Update `parse_file_uses_codex_context_for_token_count_events` to call `parse_file(&path, Provider::Codex)` and assert `scan.events[0].provider == Provider::Codex`. Update `src/dedupe.rs`'s test `event()` helper and every other test `UsageEvent { .. }` literal to add `provider: Provider::Claude` (compiler lists them all).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli record::tests::claude_roots_never` — Expected: compile failure (no `provider` arg/field), then behavioral failure.

- [ ] **Step 3: Implement**

```rust
impl LineParser {
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
                self.parse_codex_token_count(&raw)?.ok_or(SkipReason::NotAssistant)
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
}
```

`parse_claude_record(raw: RawRecord)` consumes: move `message`, `model`, ids, `session_id` (restores HEAD's zero-clone path); it returns `Result<UsageEvent, SkipReason>` directly (no `Option`), stamping `provider: Provider::Claude`. Codex/OpenAI paths stamp `Provider::Codex` / `Provider::External`. The public stateless helper:

```rust
/// Parse one line with no cross-line state, sniffing all supported formats
/// (the `External` rules). Codex token_count lines need [`parse_file`]'s
/// stateful context and will skip here with `MissingModel`-style reasons.
pub fn parse_line(line: &str) -> Result<UsageEvent, SkipReason> {
    LineParser { provider: Provider::External, ..LineParser::default() }.parse_line(line)
}
```

(`LineParser::default()` must set `provider: Provider::External`; implement `Default` manually.) `parse_file(path, provider)` passes the provider through `LineParser::new(path, provider)`. `src/scan.rs` calls `record::parse_file(&file.path, file.provider)`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: green.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add -A src && git commit -m "feat(record): dispatch parsers by root provider, stamp events

parse_file takes the root's Provider: Claude roots run only the Claude
parser (foreign usage keys can no longer fabricate events or skew skip
stats), Codex roots only the Codex parser, and --dir External roots keep
format sniffing. Claude parsing consumes the record by value again,
restoring the zero-clone hot path. Events carry the provider of the
format that parsed them.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 5: Codex fixes — dedup key, `codex-unknown` model, heartbeats, encoded cwd, label allocations

**Files:**
- Modify: `src/record.rs`
- Test: `src/record.rs` `mod tests`

**Interfaces:**
- `pub fn encode_project(path: &str) -> String` in `src/discover.rs` — Claude's observed encoding: `path.replace(['/', '.'], "-")`. (Lives in discover so scan/tui can reuse; record.rs imports it.)
- Codex dedup key: `codex:{file_stem}:{usage_index}` (file stem falls back to `"(unknown)"`). Session/turn labels no longer feed the key; `codex_turn_id` field is deleted.
- Codex model fallback chain ends in `"codex-unknown"` (never `MissingModel`).
- `token_count` with `info` null/missing or without `last_token_usage` → `Ok(None)` (counted `NotAssistant`), not `MissingUsage`.
- `codex_project` stores the ENCODED cwd (`encode_project(cwd)`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn codex_dedup_keys_do_not_collide_across_files() {
    // Two rollout files for one resumed session, no turn ids: distinct
    // events must both survive the cross-file Deduper.
    let dir = tempfile::tempdir().unwrap();
    let meta = r#"{"type":"session_meta","timestamp":"2026-07-08T01:00:00Z","payload":{"id":"sess-r","session_id":"sess-r","cwd":"/Users/v/Projects/gsd"}}"#;
    let turn = r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:01Z","payload":{"model":"gpt-5.5","cwd":"/Users/v/Projects/gsd"}}"#;
    let count = |i, o| format!(r#"{{"type":"event_msg","timestamp":"2026-07-08T01:00:02Z","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{i},"output_tokens":{o}}}}}}}}}"#);
    std::fs::write(dir.path().join("rollout-a.jsonl"), [meta, turn, &count(1000, 100)].join("\n")).unwrap();
    std::fs::write(dir.path().join("rollout-b.jsonl"), [meta, turn, &count(2000, 999)].join("\n")).unwrap();
    let a = parse_file(&dir.path().join("rollout-a.jsonl"), Provider::Codex).unwrap();
    let b = parse_file(&dir.path().join("rollout-b.jsonl"), Provider::Codex).unwrap();
    assert_ne!(a.events[0].dedup_key, b.events[0].dedup_key);
}

#[test]
fn token_counts_without_turn_context_use_codex_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-old.jsonl");
    std::fs::write(&path, [
        r#"{"type":"session_meta","timestamp":"2026-07-08T01:00:00Z","payload":{"id":"s","session_id":"s","cwd":"/Users/v/Projects/gsd"}}"#,
        r#"{"type":"event_msg","timestamp":"2026-07-08T01:00:02Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":10}}}}"#,
    ].join("\n")).unwrap();
    let scan = parse_file(&path, Provider::Codex).unwrap();
    assert_eq!(scan.stats.events, 1);
    assert_eq!(scan.events[0].model, "codex-unknown");
}

#[test]
fn null_info_heartbeats_are_other_record_types() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-hb.jsonl");
    std::fs::write(&path, r#"{"type":"event_msg","timestamp":"2026-07-08T01:00:02Z","payload":{"type":"token_count","info":null,"rate_limits":{}}}"#).unwrap();
    let scan = parse_file(&path, Provider::Codex).unwrap();
    assert_eq!(scan.stats.missing_usage, 0);
    assert_eq!(scan.stats.not_assistant, 1);
}

#[test]
fn codex_projects_use_claude_encoding() {
    // encode_project unit behavior plus the parse-time application.
    assert_eq!(crate::discover::encode_project("/Users/v/Projects/gsd"), "-Users-v-Projects-gsd");
    assert_eq!(crate::discover::encode_project("/Users/v/dot.dir/x"), "-Users-v-dot-dir-x");
}
```

Update `parse_file_uses_codex_context_for_token_count_events`: project expectation becomes `"-Users-v-Projects-openai"`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli record::tests::codex_dedup record::tests::token_counts_without record::tests::null_info record::tests::codex_projects` — Expected: FAIL (identical keys / MissingModel / missing_usage=1 / fn not found).

- [ ] **Step 3: Implement**

In `src/discover.rs`:

```rust
/// Replicate Claude Code's project-directory encoding ('/' and '.' become
/// '-'). Lossy by design; applied to Codex cwd values so one repo shows as
/// one project row regardless of provider.
pub fn encode_project(path: &str) -> String {
    path.replace(['/', '.'], "-")
}
```

In `src/record.rs` `parse_codex_token_count`:

```rust
        let Some(last_usage) = payload
            .info
            .as_ref()
            .and_then(|info| info.last_token_usage.as_ref())
        else {
            return Ok(None); // rate-limit heartbeat, not a usage record
        };
        let usage = last_usage.openai_token_usage().ok_or(SkipReason::MissingUsage)?;
        let timestamp = /* unchanged */;
        let model = self
            .codex_model
            .clone()
            .or_else(|| payload.model.clone())
            .unwrap_or_else(|| "codex-unknown".to_owned());
        self.usage_index += 1;
        let file_stem = self.fallback_session_id.as_deref().unwrap_or("(unknown)");
        let dedup_key = DedupKey::Uuid(format!("codex:{file_stem}:{}", self.usage_index));
```

`capture_codex_context` stores `Some(encode_project(cwd))` for both record types; delete `codex_turn_id` and its assignment; rename `codex_usage_index` → `usage_index` (shared with Task 6's OpenAI counter).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: green.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add -A src && git commit -m "fix(record): file-scoped codex dedup keys, codex-unknown model, encoded cwd

Dedup keys become codex:{file_stem}:{index} so resumed sessions across
rollout files cannot collide and silently collapse real usage. Files with
no turn_context count under the zero-rated codex-unknown model instead of
losing all tokens to MissingModel. info:null rate-limit heartbeats count
as other record types, and Codex cwd projects use Claude's dash encoding
so one repo is one project row.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 6: OpenAI identity and sessions — file-stem sessions, synthesized ids, BOM

**Files:**
- Modify: `src/record.rs`
- Test: `src/record.rs` `mod tests`

**Interfaces:**
- `parse_openai_record` becomes `fn parse_openai_record(&mut self, raw: &RawRecord) -> Result<Option<UsageEvent>, SkipReason>` on `LineParser` (needs `fallback_session_id` + `usage_index`).
- `session_id` = file stem (`fallback_session_id`), NOT the response id. Stateless `parse_line` has no stem → `session_id: None` (groups under `(unknown)` downstream).
- Dedup: `Uuid(id)` when `id` present; else `Uuid(format!("openai:{file_stem}:{index}"))`. `MissingIdentity` becomes unreachable for OpenAI records; doc comments on `SkipReason::MissingIdentity` and `ParseStats.missing_identity` updated to Claude-specific wording that is now accurate again.
- `parse_file` strips BOM/whitespace per line: `let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn openai_exports_group_one_session_per_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export-jan.jsonl");
    std::fs::write(&path, [
        r#"{"id":"chatcmpl-aaa","created":1783479600,"model":"gpt-5.4","usage":{"prompt_tokens":10,"completion_tokens":1}}"#,
        r#"{"id":"chatcmpl-bbb","created":1783479660,"model":"gpt-5.4","usage":{"prompt_tokens":20,"completion_tokens":2}}"#,
    ].join("\n")).unwrap();
    let scan = parse_file(&path, Provider::External).unwrap();
    assert_eq!(scan.stats.events, 2);
    assert!(scan.events.iter().all(|e| e.session_id.as_deref() == Some("export-jan")));
    assert_ne!(scan.events[0].dedup_key, scan.events[1].dedup_key);
}

#[test]
fn idless_openai_records_are_counted_with_synthesized_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("redacted.jsonl");
    std::fs::write(&path, [
        r#"{"created_at":1783476000,"model":"gpt-5.4","usage":{"input_tokens":100,"output_tokens":50}}"#,
        r#"{"created_at":1783476060,"model":"gpt-5.4","usage":{"input_tokens":200,"output_tokens":60}}"#,
    ].join("\n")).unwrap();
    let scan = parse_file(&path, Provider::External).unwrap();
    assert_eq!(scan.stats.events, 2);
    assert_eq!(scan.stats.missing_identity, 0);
    assert_ne!(scan.events[0].dedup_key, scan.events[1].dedup_key);
}

#[test]
fn bom_prefixed_first_line_parses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bom.jsonl");
    std::fs::write(&path, "\u{FEFF}{\"id\":\"resp_1\",\"created_at\":1783476000,\"model\":\"gpt-5.4\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}\n").unwrap();
    let scan = parse_file(&path, Provider::External).unwrap();
    assert_eq!(scan.stats.events, 1);
    assert_eq!(scan.stats.malformed, 0);
}
```

Update the existing OpenAI tests that asserted `session_id == Some("resp_1")` (stateless `parse_line` now yields `session_id: None`; assert that instead).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli record::tests::openai_exports record::tests::idless record::tests::bom_prefixed` — Expected: FAIL (per-response sessions / MissingIdentity skips / malformed first line).

- [ ] **Step 3: Implement** — move the function onto `LineParser`, replace the session/identity tail:

```rust
        let model = raw.model.clone().ok_or(SkipReason::MissingModel)?;
        self.usage_index += 1;
        let dedup_key = match raw.id.clone() {
            Some(id) => DedupKey::Uuid(id),
            None => {
                let stem = self.fallback_session_id.as_deref().unwrap_or("(unknown)");
                DedupKey::Uuid(format!("openai:{stem}:{}", self.usage_index))
            }
        };
        Ok(Some(UsageEvent {
            timestamp,
            session_id: self.fallback_session_id.clone(),
            project: String::new(),
            model,
            usage,
            cost_usd: raw.cost_usd,
            cost: rust_decimal::Decimal::ZERO,
            provider: Provider::External,
            dedup_key,
        }))
```

And in `parse_file`'s loop replace the current trim with the BOM-aware trim from Interfaces.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: green.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add src/record.rs && git commit -m "fix(record): file-stem sessions and synthesized ids for OpenAI exports

One export file is one session instead of one session per response line;
id-less (redacted) records get openai:{file_stem}:{index} dedup keys
instead of vanishing as MissingIdentity; a leading UTF-8 BOM no longer
costs the first record of Windows-written exports.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 7: `EventFilter.provider` + scan prefilter for Claude files + borrow-first project filter

**Files:**
- Modify: `src/scan.rs`
- Test: `src/scan.rs` `mod tests`

**Interfaces:**
- `EventFilter` gains `pub provider: Option<Provider>` (all existing literals updated with `provider: None`).
- `scan(roots: &[SearchRoot], filter)` prefilters ONLY Claude-provider files by `filter.project` before parsing; Codex/External files always parse.
- `scan_files` filter order per event: project (borrow-compare, clone only when kept), model, provider (`filter.provider.is_none_or(|p| event.provider == p)`).
- Module doc + `doctor()` doc rewritten to state: project filters skip whole files for Claude roots (path-authoritative) and apply per event for metadata-project providers; doctor's summary counters under `--project` therefore cover matching Claude files plus all Codex/External files.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn project_filter_skips_nonmatching_claude_files_but_parses_codex() {
    // Fixture: two Claude project dirs + one codex-layout root.
    // (Build with the existing fixture helpers in this module; the codex
    // file carries cwd /Users/v/Projects/alpha via session_meta.)
    // Expect: files_scanned == matching claude files + all codex files.
}

#[test]
fn provider_filter_selects_events() {
    // Claude + codex fixtures in one scan; EventFilter{provider: Some(Provider::Codex)}
    // yields only codex events.
}
```

Write these against the existing fixture helpers in `src/scan.rs` tests (`write_claude_file`-style helpers already exist; follow the current `scan()` test as the template — it builds temp roots and asserts `outcome.summary.files_scanned`). The first test replaces the current `files_scanned == 3` assertion test.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli scan::` — Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
pub fn scan(roots: &[SearchRoot], filter: EventFilter<'_>) -> ScanOutcome {
    let files: Vec<_> = discover::discover(roots)
        .into_iter()
        .filter(|file| {
            file.provider != Provider::Claude
                || filter.project.is_none_or(|project| file.project.contains(project))
        })
        .collect();
    scan_files(files, filter)
}
```

In `scan_files`'s event loop:

```rust
        for mut event in file_scan.events {
            let project: &str = if event.project.is_empty() { &file.project } else { &event.project };
            if let Some(wanted) = filter.project && !project.contains(wanted) {
                continue;
            }
            if let Some(model) = filter.model && !event.model.contains(model) {
                continue;
            }
            if let Some(provider) = filter.provider && event.provider != provider {
                continue;
            }
            if event.project.is_empty() {
                event.project = file.project.clone();
            }
            deduper.insert(event);
        }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: green.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add src/scan.rs && git commit -m "feat(scan): provider filter, Claude file prefilter, borrow-first project match

Restores whole-file --project skipping for Claude roots (path-authoritative
projects), keeps event-level filtering for Codex/External metadata
projects, adds EventFilter.provider, and stops cloning project strings for
events the filter drops. Module and doctor docs describe the actual
filtering scope again.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 8: `tycho live` consistency (event-level project filter)

**Files:**
- Modify: `src/tui/mod.rs` (`compute_snapshot`)
- Test: `src/tui/mod.rs` or `src/scan.rs` tests (wherever `compute_snapshot` is testable; it is `pub(crate)` — follow the existing test placement for it, or add a test module in `src/tui/mod.rs`)

**Interfaces:**
- Consumes: `EventFilter { project, model, provider }` from Task 7; `TranscriptFile.provider` from Task 1.
- `compute_snapshot` applies the same rule as `scan()`: Claude-provider files may be prefiltered by path project; all other files are parsed and events are filtered via `EventFilter` (which now receives the project).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn live_snapshot_matches_scan_for_codex_projects() {
    // Codex fixture with session_meta cwd /Users/v/Projects/gsd under a
    // date-sharded root; --project gsd must include its events in the
    // snapshot exactly as scan() does.
    let dir = tempfile::tempdir().unwrap();
    let day = dir.path().join("2026/07/08");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("rollout-a.jsonl"), [
        r#"{"type":"session_meta","timestamp":"2026-07-08T01:00:00Z","payload":{"id":"s","session_id":"s","cwd":"/Users/v/Projects/gsd"}}"#,
        r#"{"type":"turn_context","timestamp":"2026-07-08T01:00:01Z","payload":{"model":"gpt-5.5","cwd":"/Users/v/Projects/gsd"}}"#,
        r#"{"type":"event_msg","timestamp":"2026-07-08T01:00:02Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":50}}}}"#,
    ].join("\n")).unwrap();
    let roots = vec![SearchRoot { path: dir.path().to_path_buf(), provider: Provider::Codex }];
    let snapshot = compute_snapshot(&roots, Some("gsd"), None, /* other existing args */);
    // Assert the snapshot's total tokens for today include the 1050 tokens
    // (adapt to the snapshot struct's actual field names in this module).
}
```

Match `compute_snapshot`'s real signature when writing the test — read the function first; the essential assertion is nonzero Codex tokens under a `--project` value that only matches the cwd.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tycho-cli tui::` — Expected: FAIL (zero tokens: file pre-filtered out).

- [ ] **Step 3: Implement** — in `compute_snapshot`, replace the file filter and `EventFilter` construction:

```rust
    let files: Vec<_> = discover::discover(roots)
        .into_iter()
        .filter(|file| {
            file.provider != Provider::Claude
                || project.is_none_or(|p| file.project.contains(p))
        })
        .collect();
    // mtime collection unchanged
    let outcome = scan::scan_files(files, EventFilter { project, model, provider });
```

(`provider` here is the CLI-level provider filter threaded through in Task 9; until Task 9 lands pass `provider: None`.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test` — Expected: green.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add src/tui/mod.rs && git commit -m "fix(tui): live applies the project filter per event like scan()

live --project no longer silently drops Codex sessions whose project
lives in cwd metadata; Claude files keep the cheap whole-file skip.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 9: Global `--provider` flag; blocks defaults to Claude

**Files:**
- Modify: `src/cli.rs` (global args), `src/main.rs` (filter construction + blocks default), `src/blocks.rs` (module doc only)
- Test: `src/scan.rs` tests (provider filter already covered); add a CLI-parse test in `src/cli.rs` tests if the module has them; blocks default asserted in `src/main.rs`'s filter-construction helper test if present, else a small unit test on the helper you extract.

**Interfaces:**
- Clap: `#[arg(long, global = true, value_enum)] pub provider: Option<ProviderArg>` with

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderArg {
    Claude,
    Codex,
    Openai,
}
```

- Mapping: `Claude → Provider::Claude`, `Codex → Provider::Codex`, `Openai → Provider::External`.
- Blocks default: extract a pure helper in `src/main.rs`:

```rust
/// Blocks mirrors Claude's 5-hour usage-limit windows, so it defaults to
/// Claude events unless the user widens it with --provider.
fn effective_provider(command_is_blocks: bool, arg: Option<ProviderArg>) -> Option<Provider> {
    match (command_is_blocks, arg) {
        (_, Some(arg)) => Some(map_provider_arg(arg)),
        (true, None) => Some(Provider::Claude),
        (false, None) => None,
    }
}
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn blocks_defaults_to_claude_provider() {
    assert_eq!(effective_provider(true, None), Some(Provider::Claude));
    assert_eq!(effective_provider(false, None), None);
    assert_eq!(effective_provider(true, Some(ProviderArg::Codex)), Some(Provider::Codex));
    assert_eq!(effective_provider(false, Some(ProviderArg::Openai)), Some(Provider::External));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p tycho-cli blocks_defaults` — Expected: compile FAIL.

- [ ] **Step 3: Implement** — add the flag with doc `/// Only count events from one provider (blocks defaults to claude).`, the mapping fn, wire `EventFilter { provider: effective_provider(...), .. }` in `main.rs` for every report path and the live path; update `blocks.rs` module doc to note the Claude-only default and `--provider` widening.

- [ ] **Step 4: Run to verify pass** — `cargo test` green; manual sanity: `cargo run -- blocks --json | head -5`.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add -A src && git commit -m "feat(cli): global --provider filter; blocks defaults to Claude events

Blocks keeps meaning what the README defines (Claude 5-hour usage-limit
windows) now that Codex/OpenAI events exist; --provider claude|codex|openai
narrows any report and widens blocks.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 10: Pricing — boundary-aware lookup, zero-rated/local doctor lists, table cleanup

**Files:**
- Modify: `src/pricing.rs` (`lookup`), `src/cost.rs` (`unknown_models`, new fns), `src/scan.rs` (`doctor`), `src/report/table.rs`, `src/report/json.rs`, `src/main.rs` (warning loop), `pricing/default.toml`
- Test: `src/pricing.rs`, `src/cost.rs` tests

**Interfaces:**
- `PricingTable::lookup`: a key matches when `model == key` or (`model.starts_with(key)` and the next byte is `b'-'`); longest match wins. (No `:`-suffixed keys remain in the table.)
- `pub fn zero_rated_models(events, table) -> Vec<String>` — observed models whose matched entry has all five rates zero.
- `pub fn local_models(events, table) -> Vec<String>` — observed models with NO entry whose id contains `':'`; excluded from `unknown_models` (which keeps warning).
- Doctor output gains rows "Zero-rated models" / "Local models" and additive JSON fields `zero_rated_models`, `local_models`.
- `pricing/default.toml`: delete `gpt-5.1-codex-max`, `gemma4:`, `qwen3.6:` stanzas; add `codex-unknown` (all-zero) and `gpt-5-chat-latest` (same rates as `chat-latest`); header comment documents boundary matching and the `-codex`-variant caveat.

- [ ] **Step 1: Write the failing tests**

```rust
// src/pricing.rs
#[test]
fn lookup_requires_a_dash_boundary() {
    let table = PricingTable::default_table();
    assert!(table.lookup("claude-opus-4-8-20270101").is_some()); // dated id, '-' boundary
    assert!(table.lookup("gpt-5.41").is_none());                 // no boundary: must not match gpt-5.4
    assert!(table.lookup("gpt-5.1-codex-max").is_some());        // '-' boundary onto gpt-5.1-codex
    assert!(table.lookup("codex-unknown").is_some());
    assert!(table.lookup("gpt-5-chat-latest").is_some());
    assert!(table.lookup("gemma4:12b").is_none());               // stanza deleted; handled as local
}

// src/cost.rs
#[test]
fn local_and_zero_rated_models_are_split_out() {
    let table = PricingTable::default_table();
    let events = vec![
        test_event("qwen3.6:27b"),    // local: ':' id, no entry, no warning
        test_event("gpt-5.2-codex"),  // zero-rated entry
        test_event("brand-new-model"),// unknown: warns
    ];
    assert_eq!(local_models(&events, &table), vec!["qwen3.6:27b"]);
    assert_eq!(zero_rated_models(&events, &table), vec!["gpt-5.2-codex"]);
    assert_eq!(unknown_models(&events, &table), vec!["brand-new-model"]);
}
```

(Reuse this module's existing test-event helper; add `provider: Provider::Claude` per Task 4. Adapt `default_table()` to the real constructor name used by existing tests.)

Update the existing pricing test list: remove `gemma4:12b`, `qwen3.6:27b`, `gpt-5.1-codex-max` from the must-resolve list, add `codex-unknown`, `gpt-5-chat-latest`.

- [ ] **Step 2: Run to verify failure** — `cargo test -p tycho-cli pricing:: cost::` — Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
    pub fn lookup(&self, model: &str) -> Option<&ModelPricing> {
        self.models
            .iter()
            .filter(|(key, _)| {
                model == key.as_str()
                    || (model.starts_with(key.as_str())
                        && model.as_bytes().get(key.len()) == Some(&b'-'))
            })
            .max_by_key(|(key, _)| key.len())
            .map(|(_, pricing)| pricing)
    }
```

`cost.rs`: implement the two fns mirroring `unknown_models`'s shape (dedup + sort like it does); change `unknown_models` to skip ids containing `':'`. `scan::doctor` and the report renderers add the two lists (JSON fields additive). `main.rs` warning loop unchanged (it consumes `unknown_models`, which now excludes local ids). TOML edits per Interfaces, including the header-comment note:

```toml
# Model lookup requires a '-' boundary after a prefix key ("gpt-5.4" matches
# "gpt-5.4-mini" ids but never "gpt-5.41"). Same-family variants with their
# own rates (e.g. a future gpt-5.5-codex) still need explicit entries —
# a bare "gpt-5.5" entry would otherwise price them at API rates.
```

- [ ] **Step 4: Run to verify pass** — `cargo test` green; sanity: `cargo run -- doctor | grep -A2 -i zero`.

- [ ] **Step 5: Commit**

```bash
cargo fmt && git add -A src pricing && git commit -m "feat(pricing): boundary-aware lookup and zero-rated/local doctor lists

Prefix matches now require a '-' boundary; doctor separates explicitly
zero-rated models from unpriced ones and lists Ollama-style name:tag ids
as local (no warning) instead of shipping per-family zero stanzas. Adds
codex-unknown and gpt-5-chat-latest entries; deletes the dead
gpt-5.1-codex-max stanza and the gemma4:/qwen3.6: prefixes.

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 11: Documentation sweep

**Files:**
- Modify: `README.md`, `docs/SCHEMA.md`, `src/cli.rs` (`--project` help), `src/record.rs` + `src/scan.rs` + `src/blocks.rs` doc comments not already fixed in earlier tasks
- Test: none (docs); `cargo test` still gates the commit

- [ ] **Step 1: README** — data-sources section: note `--dir` directories are format-sniffed and should contain only usage logs; blocks section: Claude-only default + `--provider`; caveats: replace the zero-rate wording with the doctor zero-rated/local lists story.
- [ ] **Step 2: SCHEMA.md** — provider-per-root table (Claude/Codex/External); `codex-unknown` semantics; dedup key schemes (`codex:{file_stem}:{index}`, `openai:{file_stem}:{index}`); cwd dash encoding; timestamp bounds (RFC3339 or epoch seconds 2000–2100); remove the nested-response caveat; note `chat-latest` was not locally observed and `gpt-5-chat-latest` is covered.
- [ ] **Step 3: Doc comments** — `cli.rs --project`: "matches the encoded project name (Claude directory name or dash-encoded Codex cwd)"; verify `UsageEvent::project`, `SkipReason`, `ParseStats`, `scan.rs` module doc all match the shipped behavior (earlier tasks did most; this step is the audit).
- [ ] **Step 4: Verify** — `cargo test` green; `git diff --stat` shows docs only (plus help text).
- [ ] **Step 5: Commit**

```bash
git add -A README.md docs src && git commit -m "docs: describe provider-tagged scanning, filters, and pricing diagnostics

Claude-Session: https://claude.ai/code/session_01QuCVYgKZyf4A68qWdjnyh8"
```

---

### Task 12: Real-corpus verification

**Files:** none (verification only; fix regressions if found)

- [ ] **Step 1:** `cargo test` and `cargo clippy --all-targets -- -D warnings` — green.
- [ ] **Step 2:** `cargo run --release -- doctor` against the real default roots. Expected deltas vs the pre-fix baseline recorded in the design spec: malformed drops by ~778 in the affected Claude project (dual-key fix), Missing model 616 → 0 and Missing usage 123 → 0 for the Codex tree, Codex totals rise by roughly 27.4M tokens, zero-rated/local lists populated (gpt-oss:20b, qwen3:latest under local).
- [ ] **Step 3:** `cargo run --release -- projects --json | head` — no "2026"-style project rows; Codex cwd rows use dash encoding.
- [ ] **Step 4:** `cargo run --release -- blocks --json | head` vs `--provider codex` — blocks default excludes Codex events.
- [ ] **Step 5:** `CODEX_HOME= cargo run --release -- doctor | grep -i "search root"` — shows `~/.codex/sessions`, not a relative path.
- [ ] **Step 6:** Record the observed numbers in the final commit message if any docs need a truth-up; otherwise no commit from this task.
