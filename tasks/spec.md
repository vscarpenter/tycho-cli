# Spec: Pi provider

Status: approved 2026-08-31 · Tier: non-trivial

## Goal

Count usage that the Pi coding agent records locally, so spend routed
through Pi (Ollama, OpenAI, and Bedrock models alike) appears in tycho's
reports alongside Claude Code and Codex.

Today Pi's transcripts are invisible for two independent reasons: their
directory is not a default search root, and their line shape matches none
of the three formats `Provider::External` sniffs.

## Inputs

Pi writes newline-delimited JSON to
`<PI_CODING_AGENT_DIR | ~/.pi/agent>/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl`.

The env var and the layout come from Pi's own bundle: `getAgentDir()`
returns `process.env.PI_CODING_AGENT_DIR` when set, else
`join(homedir(), ".pi", "agent")`; `getSessionsDir()` appends `sessions`.
The per-project directory is `--${cwd sans leading slash, with / \ : -> -}--`.

Three record types matter:

| `type` | Carries |
|---|---|
| `session` | `id` (session uuid), `cwd`, `timestamp` |
| `message` | `message.role`; when `assistant`, also `model` and `usage` |
| everything else | nothing tycho needs (`model_change`, `thinking_level_change`) |

An assistant record:

```json
{"type":"message","id":"182b565f","timestamp":"2026-05-02T03:15:28.140Z",
 "message":{"role":"assistant","model":"us.anthropic.claude-opus-4-6-v1",
  "usage":{"input":3,"output":46,"cacheRead":0,"cacheWrite":7491,
           "totalTokens":7540,
           "cost":{"input":1.5e-05,"output":0.00115,"cacheRead":0,
                   "cacheWrite":0.04681875,"total":0.04798375}}}}
```

## Outputs

One `UsageEvent` per assistant record, tagged `Provider::Pi`.

| `UsageEvent` field | Source |
|---|---|
| `timestamp` | top-level `timestamp` (RFC3339) |
| `session_id` | `session` record's `id`, else the file stem |
| `project` | `encode_project(session.cwd)`, else empty (scan backfills `(pi)`) |
| `model` | `message.model` |
| `usage.input` | `message.usage.input` |
| `usage.output` | `message.usage.output` |
| `usage.cache_write_5m` | `message.usage.cacheWrite` |
| `usage.cache_write_1h` | always 0 |
| `usage.cache_read` | `message.usage.cacheRead` |
| `cost_usd` | `message.usage.cost.total` |
| `dedup_key` | `Uuid("pi:{session_id}:{id}")` |

## Constraints

1. **Timestamp must come from the top-level field.** `message.timestamp` is
   epoch *milliseconds* (`1777691725838`), which exceeds `EPOCH_MAX` in
   `bounded_epoch` and yields `None` — every event would drop as
   `MissingTimestamp`.
2. **Dedup key must be namespaced by session.** Pi's top-level `id` is 8 hex
   characters (32 bits); across a large corpus bare use invites birthday
   collisions, and a false dedup silently deletes spend.
3. **Pi usage field names are added to `RawUsage` as distinct fields, not
   serde aliases.** Aliasing `input` onto `input_tokens` would let a bare
   `input` key match in every format and weaken the discrimination the
   External sniffer depends on. `RawUsage` already carries both
   `input_tokens` and `prompt_tokens` for two formats; this follows that.
4. **`cacheWrite` lands wholly in `cache_write_5m`.** Pi records no TTL
   split. 5-minute is Anthropic's default and the cheaper rate (1.25x vs
   2x base input), so this under-states rather than over-states cost.
   Same convention `claude_token_usage` already applies to a
   `cache_creation_input_tokens` total with no `cache_creation` split.
5. **No tilde expansion on `PI_CODING_AGENT_DIR`**, matching `codex_home`.
6. `default_roots` stays pure — env vars are read at the `main.rs` call site.
7. Pi contributes exactly one root. It has no `archived_sessions` analogue.

## Edge cases

| Case | Behavior |
|---|---|
| `type` is not `message` (e.g. `model_change`) | `SkipReason::NotAssistant` |
| `message.role` is `user` / `toolResult` | `SkipReason::NotAssistant` |
| assistant record with no `usage` | `SkipReason::MissingUsage` |
| assistant record with no `model` | `SkipReason::MissingModel` |
| no `session` record before the first message | `project` empty; scan backfills `(pi)` |
| no `session` record at all | `session_id` falls back to the file stem |
| assistant record with no top-level `id` | `Uuid("pi:{session}:{index}")` |
| same short `id` in two different sessions | both survive dedup (constraint 2) |
| `usage.cost` absent | `cost_usd` is `None`; the pricing table applies |
| Ollama-style `model` (`glm-5.3:cloud`) | already classified *local* by `cost::local_models`; $0, no unpriced warning |

## Out of scope

- Teaching the External (`--dir`) sniffer the Pi shape.
- Stripping Bedrock region prefixes (`us.` / `eu.` / `apac.`) in the pricing
  table so `us.anthropic.claude-opus-4-6-v1` resolves under `--mode calculate`.
  Under the default `Auto` mode Pi's own `cost.total` covers it.
- Meta Muse. Its event-sourced log needs a separate design.
- Changing `blocks`' Claude-only default. The 5-hour reset is a Claude
  mechanic; `--provider pi` opts in explicitly.

## Acceptance criteria

1. `default_roots` returns `<pi_agent_dir | ~/.pi/agent>/sessions` tagged
   `Provider::Pi`, and honors `PI_CODING_AGENT_DIR` when set.
2. An empty `PI_CODING_AGENT_DIR` falls back to `~/.pi/agent`.
3. A Pi assistant line parses into a `UsageEvent` with the mapping above.
4. Non-assistant Pi lines skip as `NotAssistant`, not `Malformed`.
5. A `session` record's `cwd` sets `project` via `encode_project`; files with
   no `cwd` fall back to the `(pi)` placeholder.
6. `cacheWrite` lands in `cache_write_5m`; `cache_write_1h` stays 0.
7. `cost_usd` carries `usage.cost.total`, so `CostMode::Auto` prices a
   Bedrock-prefixed model the table cannot resolve.
8. Two assistant records sharing a short `id` across different sessions both
   survive deduplication.
9. `--provider pi` filters to Pi events; `blocks` still defaults to Claude.
10. A Pi line under a Claude or Codex root does **not** parse as usage.
11. `docs/SCHEMA.md` documents the Pi layout, records, and TTL caveat.
12. `docs/adr/0003-pi-provider.md` records the first-class-variant decision.

## Test stubs

`src/discover.rs`
- `default_roots_include_the_pi_sessions_root`
- `pi_agent_dir_env_replaces_the_default_pi_root`
- `empty_pi_agent_dir_falls_back_to_home_pi_agent`
- `pi_root_files_get_placeholder_project`

`src/record.rs`
- `parses_a_realistic_pi_assistant_record`
- `pi_session_record_supplies_project_and_session_id`
- `pi_cache_write_lands_in_the_5m_bucket`
- `pi_cost_total_populates_cost_usd`
- `pi_non_message_records_skip_as_not_assistant`
- `pi_user_and_tool_result_messages_skip_as_not_assistant`
- `pi_assistant_record_without_usage_skips`
- `pi_dedup_key_is_namespaced_by_session`
- `a_pi_line_under_a_claude_root_is_not_usage`

`src/main.rs`
- `blocks_defaults_to_claude_provider` (extend with `ProviderArg::Pi`)

`tests/cli.rs`
- integration: a Pi fixture tree reports through `--provider pi`

## Verification

`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
— run bare, never piped (see `tasks/lessons.md`).
