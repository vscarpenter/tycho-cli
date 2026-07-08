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
`session_meta` and `turn_context` records.

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
| `turn_context` | `payload.model`, `payload.cwd`, `payload.turn_id` |

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

## OpenAI API response records

Standalone JSON/JSONL response logs are parsed when they expose metadata in
OpenAI's public usage shapes:

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

For both shapes, cached input is a subset of total input. There is no OpenAI
cache-write token category in these records, so cache writes stay zero.

## Accuracy caveats

- `output_tokens` can be a mid-stream snapshot (undercount); see
  [claude-code#27361](https://github.com/anthropics/claude-code/issues/27361).
  Numbers are estimates for trend analysis, not invoice reconciliation.
- Claude Code prunes transcripts per `cleanupPeriodDays`; the recon corpus
  spans only ~7 weeks. `doctor` reports the observed date span.
- Codex logs and ChatGPT desktop caches are product-local implementation
  details and may drift. Parser changes must be verified against current
  structure-only samples before claiming support.
