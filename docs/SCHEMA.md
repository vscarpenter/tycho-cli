# Transcript and response schema — observed reality

The Claude Code section was verified against real data during Phase 0 recon
(2026-07-04): 1,004 JSONL files, 549 MB, 49,810 assistant records, spanning
2026-05-17 → 2026-07-04, written by Claude Code v2.1.156–v2.1.201. (Record
timestamps reach further back than file mtimes suggest; Phase 1's full scan
corrected the span first sampled by mtime.)

The Codex/OpenAI section was added 2026-07-08 from structure-only inspection of
local Codex JSONL sessions plus OpenAI's public prompt-caching usage examples.
No message content is parsed. Per the project rule, no field is parsed unless
it appears here or in the referenced docs. If reality and this document ever
disagree, reality wins: update this file and flag it.

## Provider-tagged search roots

Every search root carries a provider tag, and parsing is dispatched by that
tag rather than re-sniffed per line — except under `External`, where each
line is sniffed:

| Root provider | Default roots | What gets parsed |
|---|---|---|
| `Claude` | `~/.claude/projects` (+ `CLAUDE_CONFIG_DIR` entries), the Xcode `CodingAssistant` location | Only `assistant` records |
| `Codex` | `$CODEX_HOME/sessions`, or `~/.codex/sessions` when unset/empty | Only `event_msg` records where `payload.type == "token_count"` |
| `External` | Any `--dir <PATH>` (repeatable; replaces the default roots entirely) | Format-sniffed per line: try the Claude `assistant` shape, then the Codex envelope, then a standalone OpenAI record — gated only on a top-level `usage` object being present |

Under an `External` root the parser tries the Claude and Codex shapes first,
then falls back to recognizing a standalone OpenAI record by the presence of a
top-level `usage` object alone. Because that last step is usage-gated rather
than shape-gated, a `--dir` directory should contain only usage logs: any
foreign JSONL line that happens to carry a `usage` object is counted as an
event. `--provider claude|codex|openai|all` filters any report by the format
that actually parsed each event, not by the root it was discovered under — a
Claude-shaped line found via `--dir` still counts as `claude`, never `openai`;
`all` applies no provider filter. `blocks` defaults to `claude` (it mirrors
Claude's 5-hour usage-limit windows); `--provider` overrides that, and
`--provider all` widens it to every provider.

## Claude Code file layout

Three transcript layouts under each root (default `~/.claude/projects/`):

```
<encoded-project>/<session-uuid>.jsonl                                    # main session
<encoded-project>/<session-uuid>/subagents/agent-<id>.jsonl               # subagent
<encoded-project>/<session-uuid>/subagents/workflows/wf_<id>/agent-<id>.jsonl  # workflow subagent
```

`<encoded-project>` is the project path with `/` and some other characters
replaced by `-` (e.g. `-Users-vinny-Projects-gsd`). Discovery must be a
recursive `*.jsonl` glob — nesting depth varies.

Also present under roots, **not** transcripts:

- `workflows/wf_*/journal.jsonl` — workflow journals, shape `{agentId, key, type}`.
  Valid JSONL, wrong record types; type-based filtering skips them naturally.
- `memory/*.md` and other non-JSONL files — excluded by the glob.

Observed roots on the recon machine: only `~/.claude/projects`. The Xcode
location (`~/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects/`)
was absent and `CLAUDE_CONFIG_DIR` unset; both remain supported per spec.

## Codex and OpenAI file layout

Codex writes JSONL sessions under:

```
~/.codex/sessions/<yyyy>/<mm>/<dd>/rollout-<timestamp>-<session-id>.jsonl
$CODEX_HOME/sessions/<yyyy>/<mm>/<dd>/rollout-<timestamp>-<session-id>.jsonl
```

The first path components are dates, not projects, so project filtering must
be event-level. Codex carries the real project in `payload.cwd` on
`session_meta` and `turn_context` records. That `cwd` is run through the same
`/`-and-`.`-to-`-` encoding Claude Code applies to its own project directory
names (see "Claude Code file layout" above), so one repository shows as a
single project row regardless of which provider wrote the events.

The macOS ChatGPT desktop app currently stores conversation data under
`~/Library/Application Support/com.openai.chat/...` as opaque `.data` files on
the inspected machine. Those files are not scanned by default. Plain JSONL logs
or exports containing OpenAI Responses API or Chat Completions response objects
can be scanned with `--dir`.

## Claude Code record types

15 `type` values observed: `assistant`, `user`, `attachment`, `last-prompt`,
`mode`, `permission-mode`, `bridge-session`, `ai-title`,
`file-history-snapshot`, `system`, `queue-operation`, `pr-link`, `started`,
`result`, `worktree-state`.

Only `assistant` records carry usage. `user` records never do (verified:
1,299/1,299 sampled user records have no `message.usage`).

Zero malformed lines and zero empty files existed in the recon corpus, but
both are still handled (skip + count) — other machines will differ.

## Assistant record envelope

All fields must be treated as optional at parse time. Observed:

| Field | Notes |
|---|---|
| `type` | `"assistant"` |
| `uuid` | record identity, dedup fallback key |
| `parentUuid` | conversation threading; not used |
| `timestamp` | ISO-8601 with `Z` (e.g. `2026-07-02T23:54:44.905Z`) |
| `sessionId` | session UUID; subagent files carry the **parent** session's id |
| `session_id` | snake_case duplicate of `sessionId` on some records |
| `requestId` | `req_...`; missing on some synthetic records |
| `cwd` | working directory; can drift mid-session (`cd`) |
| `gitBranch` | present on most modern records; not used |
| `version` | Claude Code version string |
| `isSidechain` | `true` in subagent files |
| `agentId` | subagent files only |
| `isApiErrorMessage` | `true` on synthetic error records |
| `message.id` | `msg_...` API message id; a bare UUID on synthetic records |
| `message.model` | model id, or `"<synthetic>"` |
| `message.usage` | see below |
| attribution fields | `attributionSkill`, `attributionAgent`, `attributionPlugin`, `attributionMcpServer`, `attributionMcpTool`, `advisorModel`, `slug` — present on various records; not used |

`costUSD` appeared in **zero** records in this corpus (documented in older
Claude Code versions; supported for backwards compatibility). Consequence:
`--mode auto` behaves like `calculate` on modern data, and `--mode display`
is only meaningful for legacy transcripts.

`message.content` exists in the raw data and is deliberately absent from our
types. See ADR 0001.

## Usage block

```json
{
  "input_tokens": 2,
  "output_tokens": 5301,
  "cache_creation_input_tokens": 5521,
  "cache_read_input_tokens": 196377,
  "cache_creation": {
    "ephemeral_5m_input_tokens": 5521,
    "ephemeral_1h_input_tokens": 0
  }
}
```

- The `cache_creation` TTL breakdown was present in **every** assistant
  record in the corpus, back to v2.1.156. Cost math prices 5m and 1h writes
  separately; if the object is ever missing, `cache_creation_input_tokens`
  is priced at the 5-minute rate.
- Ignored extra fields observed: `service_tier`, `speed`, `inference_geo`,
  `iterations`, `server_tool_use`.

## Duplicate records (load-bearing)

Streaming writes multiple records for one API message. Observed worst case:
**7 records** sharing one `(message.id, requestId)` within a session file.
In observed samples the duplicate usage values were identical (not ascending
snapshots), so naive summing would have counted that message 7×.

Dedup rule: identity is `(message.id, requestId)` when both present, else
`uuid`; keep the record with the greatest `output_tokens`, tie-break on the
latest timestamp. See ADR 0002.

## Synthetic records

107 files contain records with `model: "<synthetic>"` and
`isApiErrorMessage: true`: API-error placeholders with all-zero usage and a
bare-UUID `message.id`, sometimes missing `requestId`. They contribute
nothing to totals, must not trigger "unknown model" warnings, and are
counted separately by `doctor`.

## Codex token-count records

Codex JSONL records are envelope objects with top-level `type`, `timestamp`,
and `payload`. Usage is not attached to `response_item`; it appears in
`event_msg` records where `payload.type == "token_count"`.

Context records parsed before token counts:

| Record | Fields used |
|---|---|
| `session_meta` | `payload.id`, `payload.session_id`, `payload.cwd` |
| `turn_context` | `payload.model`, `payload.cwd` |

Token-count record shape:

```json
{
  "type": "event_msg",
  "timestamp": "2026-07-08T01:40:35.865Z",
  "payload": {
    "type": "token_count",
    "info": {
      "last_token_usage": {
        "input_tokens": 27780,
        "cached_input_tokens": 4992,
        "output_tokens": 588,
        "reasoning_output_tokens": 337,
        "total_tokens": 28368
      },
      "total_token_usage": {
        "input_tokens": 27780,
        "cached_input_tokens": 4992,
        "output_tokens": 588,
        "reasoning_output_tokens": 337,
        "total_tokens": 28368
      },
      "model_context_window": 258400
    }
  }
}
```

Use `last_token_usage`, not `total_token_usage`; the latter is cumulative for
the session/turn stream and would double count. OpenAI-style `input_tokens`
includes cached input, so tycho stores `input_tokens - cached_input_tokens` as
uncached input and `cached_input_tokens` as cache reads.

When a token-count record is reached before any `turn_context` has supplied a
model — a resumed session file with no context lines, for example — tycho
stamps the event's model as the synthetic `codex-unknown` id instead of
skipping it. `codex-unknown` is zero-rated in the default pricing table, so it
never triggers an "unpriced model" warning; `doctor` lists it (and any other
zero-priced-by-design id) under "Zero-rated models", separately from models
that are genuinely unpriced.

Codex records carry no per-message id analogous to Claude's `message.id`, so
dedup identity is synthesized per file instead: `codex:{file_stem}:{index}`,
where `index` counts token-count events within that file starting at 1 and
`file_stem` falls back to `(unknown)` if the path has none. Keying by file
(not by session or turn id) keeps two independent rollout files for one
resumed session from colliding.

## OpenAI API response records

Standalone JSON/JSONL response logs are recognized under `External` roots by
one gate only: a top-level `usage` object. The `object` field and the
`resp_`/`chatcmpl_` id prefixes shown below are what real records look like,
but they are not inspected by the parser — a record with `usage` and no
recognizable `object` still parses, and a record with `object: "response"`
but no `usage` does not. One export file is treated as one session:
`session_id` is the file's stem, not a per-record id.

Responses API:

```json
{
  "id": "resp_...",
  "object": "response",
  "created_at": 1783476000,
  "model": "gpt-5.4",
  "usage": {
    "input_tokens": 1000,
    "output_tokens": 50,
    "total_tokens": 1050,
    "input_tokens_details": {
      "cached_tokens": 400
    }
  }
}
```

Chat Completions:

```json
{
  "id": "chatcmpl_...",
  "object": "chat.completion",
  "created": 1783479600,
  "model": "chat-latest",
  "usage": {
    "prompt_tokens": 1000,
    "completion_tokens": 100,
    "total_tokens": 1100,
    "prompt_tokens_details": {
      "cached_tokens": 100
    }
  }
}
```

The `chat-latest` model id above is OpenAI's own doc-example value, not
something observed in a local export; real Chat Completions logs typically
carry `gpt-5-chat-latest` instead, which the default pricing table also
covers.

For both shapes, cached input is a subset of total input. There is no OpenAI
cache-write token category in these records, so cache writes stay zero.

Dedup identity follows the record's own id when it has one: a `resp_...` or
`chatcmpl_...` id becomes the dedup key directly. Redacted exports that carry
no `id` instead get a synthesized `openai:{file_stem}:{index}` key — the same
per-file scheme Codex uses above — so those records still survive dedup
instead of vanishing as `MissingIdentity`.

## Timestamp parsing

`timestamp`, `created`, and `created_at` fields accept either an RFC3339
string (Claude's `"2026-07-02T23:54:44.905Z"` style) or an integer epoch-
seconds value, bounded to `[2000-01-01T00:00:00Z, 2100-01-01T00:00:00Z)`.
Anything else — a millisecond epoch landing in year ~58486, a float, or an
epoch written as a string (`"1234"`) — fails to parse, and the record is
skipped as `MissingTimestamp` rather than producing an implausible date.

## Accuracy caveats

- `output_tokens` can be a mid-stream snapshot (undercount); see
  [claude-code#27361](https://github.com/anthropics/claude-code/issues/27361).
  Numbers are estimates for trend analysis, not invoice reconciliation.
- Claude Code prunes transcripts per `cleanupPeriodDays`; the recon corpus
  spans only ~7 weeks. `doctor` reports the observed date span.
- Codex logs and ChatGPT desktop caches are product-local implementation
  details and may drift. Parser changes must be verified against current
  structure-only samples before claiming support.
- Historical/private Codex labels with no public API rate are explicitly
  zero-priced by default, and Ollama-style `name:tag` local model ids carry
  no pricing entry at all; `doctor` (table and `--json`, as
  `zero_rated_models`/`local_models`) lists both separately from models that
  are genuinely unpriced.
