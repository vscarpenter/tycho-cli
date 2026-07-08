# Provider-tagged parsing and OpenAI/Codex review fixes

**Date:** 2026-07-08
**Status:** Approved
**Scope:** Resolve all 20 verified findings from the code review of the
OpenAI/Codex support change (15 reported + 4 cut by the report cap + 1
plausible resolved by judgment call).

## Problem

The OpenAI/Codex support change identifies the provider of a JSONL record by
per-line content sniffing and bolts Codex/OpenAI concepts onto structures
designed for Claude's layout. The review confirmed, with end-to-end
reproductions, that this causes silent data loss (dual-key Claude records
counted malformed; no-`turn_context` Codex files dropped; empty `CODEX_HOME`
losing the whole Codex tree), silent wrongness (cross-file dedup collisions,
double-counting via an undocumented embedded-response path, year-58486 and
1970 timestamps), and incoherence (two project encodings, `live` vs `daily`
filter disagreement, year directories surfacing as projects, blocks report
semantics silently changed).

## Design

### 1. `Provider` at the discovery seam

A `Provider` enum — `Claude`, `Codex`, `External` — is assigned per search
root, never per line:

- Claude config roots (`~/.claude/projects`, `CLAUDE_CONFIG_DIR` entries,
  the Xcode location) → `Claude`
- `$CODEX_HOME/sessions` / `~/.codex/sessions` → `Codex`
- every `--dir` root → `External` (the only place format sniffing remains;
  it is the explicit opt-in for arbitrary export/log directories)

Carriers:

- `discover::default_roots` returns `Vec<SearchRoot>` where
  `SearchRoot { path: PathBuf, provider: Provider }`; `--dir` paths become
  `External` roots in `resolve_roots`.
- `TranscriptFile` gains `provider: Provider`.
- `UsageEvent` gains `provider: Provider` (internal; not added to report
  JSON except the additive doctor fields listed below).
- `EventFilter` gains `provider: Option<Provider>`.

`parse_file(path, provider)` dispatches:

- **Claude**: Claude parser only. A stray top-level `usage` key can never
  fabricate an event or skew skip stats.
- **Codex**: Codex context capture + `token_count` parser only. The
  OpenAI parser never runs inside Codex roots.
- **External**: try Claude (by `type == "assistant"`), Codex (by envelope
  `type`), then OpenAI gated **only on the presence of a top-level `usage`
  object**. `looks_like_openai_object`, the `object` field, and
  `resp_`/`chatcmpl` id sniffing are deleted.

### 2. Parsing fixes (`src/record.rs`)

- **Dual-key alias bug**: remove `alias = "session_id"` from the
  `sessionId` field (`rename_all = "camelCase"` already accepts
  `sessionId`; the documented snake_case duplicate returns to being an
  ignored unknown key). The equivalent `turnId`/`conversationId` alias-pair
  hazards disappear with the deletions below.
- **Delete speculative surface** (SCHEMA.md "no undocumented fields" rule +
  YAGNI): `conversation_id`, `thread_id` (both structs),
  `RawRecord.response`, `RawCodexPayload.response`, `RawOpenAiResponse`,
  `OpenAiCandidate`. The OpenAI parser reads `RawRecord`'s own
  `id`/`model`/`usage`/`created`/`created_at` fields directly. This is also
  the fix for the struct-triplication finding (by deletion), the
  embedded-response double-count, and the top-level-id shadowing edge.
- **Timestamps**: `RawTimestamp` becomes a newtype over `DateTime<Utc>`
  with a hand-written `Deserialize`:
  - `visit_str` → RFC3339 only (no integer-string fallback)
  - `visit_i64`/`visit_u64` → epoch **seconds**, accepted only within
    `[2000-01-01T00:00:00Z, 2100-01-01T00:00:00Z)`
  - floats rejected
  Out-of-range or unparseable values make the field `None` (skip →
  `MissingTimestamp`), not a malformed record, preserving per-field
  permissiveness. This fixes ms-epoch year-58486 dates, the `"1234"` → 1970
  regression, and removes the per-line untagged-enum String allocation.
- **No-`turn_context` Codex files**: model resolution falls back
  `codex_model` → `payload.model` → synthetic **`codex-unknown`**. Their
  tokens count; cost is $0 via a zero-rated pricing entry; doctor lists the
  model as zero-rated.
- **Codex dedup key**: `codex:{file_stem}:{index}` (per-file counter).
  File identity replaces session/turn labels, so per-file indices cannot
  collide across rollout files. Copies of the same file (same stem) still
  dedupe.
- **OpenAI identity and sessions**: `session_id` = file stem (as the Codex
  path already does), so one export file is one session. Records without an
  `id` get a synthesized `openai:{file_stem}:{index}` dedup key instead of
  being dropped; records with an `id` keep `Uuid(id)` so genuine duplicates
  still collapse.
- **Heartbeats**: a `token_count` whose `info` is null/missing or has no
  `last_token_usage` is classified `NotAssistant` (other record type), not
  `MissingUsage`.
- **BOM**: strip a leading U+FEFF from each line before parsing.
- **Allocation restoration**: dispatch `parse_line` on `record_type` first
  and pass the record by value to the Claude parser (restores HEAD's string
  moves); drop the needless `to_owned()` calls when formatting Codex dedup
  labels.
- Doc comments updated: `UsageEvent::project`, `SkipReason::MissingIdentity`,
  `ParseStats` fields.

### 3. Project dimension (`src/discover.rs`, `src/scan.rs`, `src/tui/`)

- **One encoding**: Codex `cwd` is encoded with Claude's observed scheme
  (`/` and `.` → `-`) at parse time. `/Users/v/Projects/gsd` and
  `-Users-v-Projects-gsd` become one project row; a pasted `--project`
  label matches both providers. The encoding function lives in one place
  and is documented in SCHEMA.md.
- **Codex path-derived project**: `project_name` is provider-aware; files
  under a Codex root get the placeholder `(codex)` (mirroring `(root)`),
  never a date component. No-cwd events aggregate under `(codex)`; `live`'s
  active-session header shows `(codex)` instead of a year.
- **Empty `CODEX_HOME`**: trim; if empty, fall back to `~/.codex` —
  mirroring the `CLAUDE_CONFIG_DIR` branch.
- **`live`/`daily` consistency**: `compute_snapshot` passes the project
  filter through `EventFilter` instead of pre-filtering files.
- **Claude file prefilter restored**: `scan()` skips non-matching
  **Claude-provider** files before parsing (their project is
  path-authoritative), restoring perf and doctor scoping for the dominant
  corpus. Codex/External files are always parsed and filtered per event.
  `scan.rs` module doc, `doctor()` doc, and `cli.rs --project` help updated
  to describe exactly this.
- Filter comparisons borrow (`&str`) before cloning; clones only for kept
  events.

### 4. Provider filter and blocks (`src/cli.rs`, `src/blocks.rs`, `src/main.rs`)

- New global `--provider <claude|codex|openai>` flag feeding
  `EventFilter.provider` (`openai` maps to `External`).
- **`blocks` defaults to `claude`** when `--provider` is not given,
  keeping the README's usage-limit semantics true. All other reports and
  `live` default to all providers.
- README blocks section and `blocks.rs` module doc note the default and
  the flag.

### 5. Pricing (`src/pricing.rs`, `src/cost.rs`, `pricing/default.toml`)

- **Zero-rated visibility**: doctor gains a "Zero-rated models" list —
  observed models whose matched pricing entry is all-zero (additive JSON
  field `zero_rated_models`). `codex-unknown` gets a zero-rate stanza. The
  dead `gpt-5.1-codex-max` stanza is deleted.
- **Local models**: an unmatched model id containing `:` (Ollama
  `name:tag`) is zero-rated silently and listed by doctor under a "Local
  models" list (additive JSON field `local_models`); it is excluded from
  the unpriced warning. The `gemma4:`/`qwen3.6:` stanzas are deleted.
- **Boundary-aware lookup**: a prefix entry matches only when the model id
  ends at the prefix or continues with a boundary character (`-`), so dated
  ids (`claude-opus-4-8-20270101`) keep resolving while `gpt-5.41` cannot
  match `gpt-5.4`. Same-family capture (`gpt-5.5-codex` matching `gpt-5.5`)
  cannot be distinguished structurally; the table header documents that
  `-codex` variants need their own entries.
- **chat-latest**: keep the stanza, add a `gpt-5-chat-latest` sibling at
  the same rates; SCHEMA.md notes the bare id was not locally observed.

### 6. Documentation

README (data sources, blocks default, caveats, a note that `--dir`
directories use format sniffing and should contain only usage logs),
SCHEMA.md (provider-per-root table, `codex-unknown`, dedup-key schemes, cwd
encoding, timestamp bounds, chat-latest provenance), and every stale doc
comment the review flagged.

## Error handling

Unchanged philosophy: parse failures skip-and-count per line, never fatal.
The new bounds (timestamps) and reclassifications (heartbeats) only move
records between skip categories or into the event stream.

## Testing

TDD throughout — each fix lands with a failing test first:

- dual-key (`sessionId` + `session_id`) assistant line parses as one event
- no-`turn_context` Codex fixture counts under `codex-unknown`
- `CODEX_HOME=""` resolves to `~/.codex/sessions`
- same fixture through `scan()` and `compute_snapshot` agrees under
  `--project`
- two rollout files sharing a session id do not collide (full totals)
- ms-epoch and integer-string timestamps are skipped, not events
- Codex cwd encodes to the Claude form; one `projects` row across providers
- Codex-root files get project `(codex)`, never a year
- `usage: null` chunks count as other-record-type
- a foreign `usage`-bearing line under a Claude root produces no event;
  the same line under an External root does
- BOM-prefixed export parses its first record
- one session per export file; id-less export records are counted
- blocks defaults to Claude-only; `--provider` filters all reports
- doctor lists zero-rated and local models; unpriced warning excludes them
- boundary-aware lookup: dated ids resolve, `gpt-5.41` does not match
  `gpt-5.4`

Final verification: `cargo test`; before/after `doctor` against the real
`~/.claude` + `~/.codex` corpora (expected deltas on this machine:
malformed 778 → 0 in the affected project, missing model 616 → 0, missing
usage 123 → 0, Codex totals rise by ~27.4M tokens).

## Trade-offs accepted

- `External` keeps content sniffing — that is the generic-export feature,
  now confined to explicit `--dir` usage and documented.
- Report JSON gains no new fields except additive doctor lists.
- Project display keeps Claude's dash encoding (it is lossy and cannot be
  decoded to real paths); consistency wins over prettiness.
- Same-family pricing prefix capture (`gpt-5.5-codex` → `gpt-5.5`) is
  documented rather than solved structurally.
