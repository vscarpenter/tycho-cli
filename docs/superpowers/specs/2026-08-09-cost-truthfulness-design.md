# Cost truthfulness: Codex model backfill, cache TTL split, shadow estimates

**Date:** 2026-08-09
**Status:** Approved
**Scope:** Three changes that recover or surface cost information tycho
already parses but currently loses before it reaches the user. No new
providers, no new parsers, no new data sources.

## Problem

A reconciliation of tycho against `ccusage` over the same corpus confirmed
tycho is correct everywhere the two disagree (Codex coverage, streaming-dedup
tie-break, 1-hour cache-write pricing). That investigation surfaced three
places where tycho itself under-reports or hides cost it has already parsed.

### 1. Codex model attribution is lost on resumed session files

`RawCodexPayload` handling stamps `codex-unknown` when a `token_count` record
is reached before any `turn_context` supplied a model (`record.rs:459-462`,
documented in `docs/SCHEMA.md:230`). `codex-unknown` is zero-rated, so the
spend silently vanishes from every cost total.

Measured on the reference corpus (`~/.codex/{sessions,archived_sessions}`):

| Metric | Value |
|---|---|
| Files producing `codex-unknown` events | 39 |
| …recoverable from a later `turn_context` | 26 |
| …genuinely unrecoverable (no `turn_context` at all) | 13 |
| Raw tokens in those events, pre-dedup | 549,172,771 |
| …recoverable, pre-dedup | 521,791,939 (95.0%) |

The two token figures above are raw sums taken directly from the JSONL, before
tycho's dedup pass; `tycho models --model codex-unknown` reports the deduped
bucket as 434,311,580 tokens (14,567,647 input / 1,879,357 output /
417,864,576 cache read) at $0. The gap is expected — a rollout present in both
`sessions/` and `archived_sessions/` shares a file stem, so its events share
the `codex:{file_stem}:{index}` dedup key and collapse. The recoverable
**share** (95.0%) is what carries over; the dollar figure below is computed
from tycho's deduped bucket, not the raw sum.

Every one of the 26 recoverable files resolves to `gpt-5.6-sol`. The 13
unrecoverable files all date to 2025-09-11/12/15 — the oldest data in the
corpus, predating the `turn_context` record.

At `gpt-5.6-sol` rates ($5 / $30 / $0.50), 95% of the deduped bucket is worth
**≈$321** currently reported as zero: $69.20 input + $53.56 output +
$198.49 cache read.

### 2. The cache TTL split is parsed and then discarded

`TokenUsage` carries `cache_write_5m` and `cache_write_1h` separately, and
the pricing table rates them differently (e.g. `claude-fable-5`: 12.5 vs
20.0). No report displays the split. On the reference corpus for July 2026
the 1-hour share was 65.9% (fable-5), 79.5% (opus-4-8), 100% (opus-5), and
the premium over the 5-minute rate came to **$232.24** — two thirds of the
month's total variance against `ccusage`.

### 3. Zero-rated labels are invisible in totals

`gpt-5-codex`, `gpt-5.1-codex`, and `gpt-5.2-codex` are deliberately
zero-rated in `pricing/default.toml` ("historical/private model labels that
do not appear on the current public API rate card"). That is the correct
out-of-pocket figure, but it means ~391M tokens contribute $0 to a headline
total with nothing marking the omission.

## Design

Chosen packaging: extend the reports that already own each concern. No new
commands and no new global flags.

### 1. Codex model backfill (`src/record.rs`, `src/scan.rs`)

`self.codex_model` is set by `turn_context` and never reverts to `None`.
Therefore every event stamped `codex-unknown` necessarily precedes the file's
**first** `turn_context`. "Nearest following model" and "first model in the
file" are provably the same value, so no per-event search is required.

- Add `first_codex_model: Option<String>` to the Codex parser state, set once
  on the first `turn_context` that carries a model. It is distinct from
  `codex_model`, which tracks the *current* model and would hold the last
  value at end-of-file.
- After the per-file parse completes, replace the model on every event whose
  model is exactly `codex-unknown` with `first_codex_model` when present.
  This runs in the existing post-parse step that already backfills `project`
  from the file's location (`record.rs:50-56`), so it is an application of an
  established pattern rather than a new mechanism.
- Backfill is strictly per-file. Parser state is already per-file and
  `scan.rs:86` processes one file per rayon task, so cross-file contamination
  is structurally impossible.
- Files with no `turn_context` at all retain `codex-unknown`, which remains
  zero-rated and continues to be listed by `doctor` under "Zero-rated
  models".

**Auditability.** `ParseStats` gains a `codex_model_backfilled: u64` counter,
surfaced by `doctor` as a `Codex models backfilled` row. This follows the
codebase's "skips are data, not errors" stance: the correction is counted and
visible, never silent.

### 2. Cache TTL split on `tycho cache` (`src/cache.rs`, `src/report/{table,json}.rs`)

**Narrative.** The report already opens with a summary paragraph. It gains
one sentence in the same voice, emitted only when at least one 1-hour cache
write exists:

```
63.4% of your cache writes used the 1-hour TTL, costing $232.24 more than the
5-minute rate.
```

The premium is defined as, summed over models:

```
premium = cache_write_1h * (rate.cache_write_1h - rate.cache_write_5m)
```

**Table.** The existing `Cache Write` column is replaced by `Write 5m` and
`Write 1h` — a net increase of one column, 8 to 9. The per-model TTL premium
is not given a column; the table already runs about 120 characters and the
5m/1h pair makes the share legible on its own.

**JSON.** Per-model and total objects gain `cache_write_5m`,
`cache_write_1h`, and `ttl_premium_usd`. `cache` has no CSV path, so no CSV
change. All JSON changes are additive, preserving the existing contract.

### 3. Shadow estimates in `tycho doctor` (`src/pricing.rs`, `src/report/{table,json}.rs`)

A shadow cost requires a rate, and inferring one silently would repeat the
error this work exists to correct. The mapping is therefore explicit,
user-visible configuration rather than inference.

`pricing/default.toml` gains an optional section:

```toml
[shadow]
"gpt-5-codex"   = "gpt-5.4"
"gpt-5.1-codex" = "gpt-5.4"
"gpt-5.2-codex" = "gpt-5.4"
```

Each key is a zero-rated or unpriced model id; each value is the id of a
priced model whose rates stand in for it. `gpt-5.4` is chosen as the
same-generation label that carries public rates. Users override the section
in `~/.config/tycho/pricing.toml` or via `--pricing`, exactly as they already
override `[models]`.

`doctor` gains two rows:

```
Unpriced tokens     391,208,320 across 3 models
Shadow estimate     $283.41 at reference rates (see [shadow] in pricing)
```

Rules:

- "Unpriced tokens" counts tokens belonging to models that resolve to
  all-zero rates or to no rate at all, excluding models classified as local
  (Ollama-style `name:tag` ids), whose $0 is a fact rather than a gap.
- A model with no `[shadow]` entry contributes to the token count and
  contributes nothing to the dollar estimate. No figure is ever guessed.
- `codex-unknown` — the residue left on files with no recoverable model — is
  itself zero-rated, so whatever survives backfill counts toward "Unpriced
  tokens". It gets no default `[shadow]` entry, because its defining property
  is that the model is unknown; it therefore contributes tokens and no
  dollars.
- When no unpriced tokens exist, both rows read `(none)` for consistency
  with the neighbouring `Models without pricing` row.
- The shadow estimate is a diagnostic only. It never enters `cost`, never
  changes any report total, and is absent from every report but `doctor`.

`doctor --json` gains `unpriced_tokens`, `shadow_estimate_usd`, and a
`shadow_mappings` object. Additive.

## Data flow

```
discover (unchanged)
  -> parse_file, per file, per provider
       Codex: capture turn_context model -> first_codex_model
              emit events (some stamped codex-unknown)
  -> per-file post-pass: backfill project (existing)
                         backfill model   (new, counted)
  -> Deduper (unchanged)
  -> Coster (unchanged; backfilled models now resolve real rates)
  -> aggregate
       cache report: sum cache_write_5m / cache_write_1h, compute premium
       doctor:       classify models, sum unpriced tokens, apply [shadow]
  -> report/{table,json,csv}
```

## Error handling

- A `[shadow]` value naming a model with no rates is a configuration error
  surfaced at pricing-load time with the offending key, consistent with how
  malformed `[models]` stanzas are handled today. It never panics and never
  silently yields zero.
- A `[shadow]` key naming a model that is priced is ignored: the real rate
  wins and no shadow figure is produced for it.
- Backfill never fails. A file with no recoverable model keeps
  `codex-unknown`.

## Testing

TDD throughout, red before green, matching the existing split of in-module
unit tests plus `tests/` integration tests.

Backfill:
- `token_count` before `turn_context` recovers the model
- a file with no `turn_context` keeps `codex-unknown`
- two `turn_context` records with different models backfill the **first**
- two files in one scan do not contaminate each other
- `codex_model_backfilled` counts exactly the events corrected

Cache TTL:
- premium arithmetic per model and summed
- narrative sentence is emitted only when `cache_write_1h > 0`
- table renders 9 columns with the write pair
- JSON carries the three new fields

Shadow:
- unpriced token count excludes local `name:tag` models
- a model with a `[shadow]` entry contributes dollars; one without
  contributes only tokens
- a `[shadow]` value naming an unknown model is a load-time error
- totals in every other report are unchanged by the presence of `[shadow]`

## Behaviour change

The backfill **changes historical numbers**: on the reference corpus the
all-time total moves by roughly +$321 as ~522M tokens gain real rates. This
is a correction, not a regression, but it must be called out in the release
notes and in the README's accuracy caveats so a user who tracks the number
month over month is not surprised.

## Out of scope

- Additional providers or local-LLM runtimes. Local models already work:
  `pricing.rs` deliberately carries no stanza for `name:tag` ids and `doctor`
  classifies them as "Local models". On the reference corpus they account for
  814,517 of 7,691,253,284 tokens (0.011%), and local inference is genuinely
  free, so more local-model plumbing would add rows rather than insight.
- A global `--shadow` flag exposing the estimate on `daily`/`monthly`/
  `models`. Deferred until `doctor` proves too quiet a home; trivial to add
  later, speculative now.
- A dedicated `tycho audit` command. `doctor` already owns "can I trust these
  numbers?" and two commands answering it would be worse than one.
- Changing the dedup policy or the 1-hour cache pricing. Both were verified
  correct during the reconciliation.
