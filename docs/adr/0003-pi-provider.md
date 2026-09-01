# 0003 — Pi is a first-class provider, not an External shape

- **Date:** 2026-08-31
- **Status:** Accepted
- **Deciders:** Vinny Carpenter

## Context

The Pi coding agent records complete usage metadata locally — model, tokens,
cache reads and writes, and its own computed cost — one JSONL file per
session under `<PI_CODING_AGENT_DIR | ~/.pi/agent>/sessions/`. None of it
reached tycho, for two independent reasons: the directory was not a default
search root, and Pi's line shape matched none of the three formats
`Provider::External` sniffs.

Pi's envelope is `{"type":"message","message":{"role":"assistant","usage":…}}`.
The Claude branch requires a top-level `type` of `"assistant"`; the OpenAI
branch reads `usage` at the top level of the line, where Pi nests it one
level down under `message`. Every line skipped as `NotAssistant`.

The gap was material. One machine's Pi sessions held 23M tokens, including
$89 of `gpt-5.5-pro` and Bedrock-routed Opus traffic, none of it counted.

## Decision

Add `Provider::Pi` as a fourth variant with its own default search root and
its own branch in `LineParser::parse_line`, rather than widening the
External sniffer to accept Pi's shape.

Three consequences of that choice are load-bearing enough to name here:

- **The dedup key is namespaced by session** — `Uuid("pi:{session}:{id}")`.
  Pi's per-record `id` is 8 hex characters. Used bare across a large corpus,
  32 bits invites birthday collisions, and under ADR 0002 a collision does
  not merely double-count — it silently *deletes* the loser's spend.
- **The timestamp comes from the top-level field, never `message.timestamp`**,
  which is epoch milliseconds and fails `bounded_epoch`'s year-2100 ceiling.
- **Pi's recorded `usage.cost.total` maps onto the existing `cost_usd`
  field**, so the default `CostMode::Auto` uses it. This is the only truthful
  source for Bedrock-prefixed ids such as
  `us.anthropic.claude-opus-4-6-v1`: `longest_prefix_match` is a prefix rule,
  so the region prefix blocks resolution onto `claude-opus-4-6` and the
  pricing table would report $0.

## Consequences

- Easier: Pi is a default root (no `--dir`), `--provider pi` filters it,
  `doctor` names it among the scanned roots, and adding the variant makes the
  compiler enumerate every match site that must handle it.
- Easier: Pi's `session` record carries `cwd`, so `encode_project` puts Pi
  work under the same project row as Claude Code and Codex work in the same
  repository — the reason cross-provider rollups are worth having.
- Harder: `Provider` is public, so the variant reaches `cli::ProviderArg`,
  `main::effective_provider`, and `doctor`'s output. A fourth arm now has to
  be considered wherever provider is matched.
- Accepted inaccuracy: Pi records a single `cacheWrite` with no TTL split, so
  all of it is priced at the 5-minute rate. `tycho cache`'s 1-hour-premium
  line therefore treats Pi writes as known-5m when they are truly unknown.
  Documented in `docs/SCHEMA.md`. The error is conservative — 5-minute is
  both Anthropic's default TTL and the cheaper rate (1.25x vs 2x base input),
  so this under-states cost rather than over-stating it.

## Alternatives

- **Widen the External sniffer instead:** rejected — smaller diff, but it
  forces `--dir` on every run, makes Pi events indistinguishable from OpenAI
  ones under `--provider`, and leaves `doctor` unable to name Pi as a source.
- **Serde-alias Pi's field names onto `RawUsage`'s existing fields:** rejected
  — aliasing `input` onto `input_tokens` would let a bare `input` key match in
  *every* format, weakening exactly the discrimination the External sniffer
  depends on. Pi's names are added as distinct fields with their own
  conversion method, the way `prompt_tokens` already sits beside
  `input_tokens`.
- **A third `cache_write_unknown` bucket in `TokenUsage`:** rejected — it
  models the truth, but `TokenUsage` is shared, so it would ripple through
  aggregation, the cache report, and the `--json` and `--csv` output shapes.
  A breaking output-contract change is too much to spend on one provider's
  metadata gap.
