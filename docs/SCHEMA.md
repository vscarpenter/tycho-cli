# Transcript schema — observed reality

Everything in this document was verified against real data during Phase 0
recon (2026-07-04): 1,004 JSONL files, 549 MB, 49,810 assistant records,
spanning 2026-05-17 → 2026-07-04, written by Claude Code v2.1.156–v2.1.201.
(Record timestamps reach further back than file mtimes suggest; Phase 1's
full scan corrected the span first sampled by mtime.)
Per the project rule, no field is parsed unless it appears here or in the
referenced docs. If reality and this document ever disagree, reality wins:
update this file and flag it.

## File layout

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

## Record types

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

## Accuracy caveats

- `output_tokens` can be a mid-stream snapshot (undercount); see
  [claude-code#27361](https://github.com/anthropics/claude-code/issues/27361).
  Numbers are estimates for trend analysis, not invoice reconciliation.
- Claude Code prunes transcripts per `cleanupPeriodDays`; the recon corpus
  spans only ~7 weeks. `doctor` reports the observed date span.
