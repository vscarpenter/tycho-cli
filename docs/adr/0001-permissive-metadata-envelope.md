# 0001 — Permissive, metadata-only deserialization envelope

- **Date:** 2026-07-04
- **Status:** Accepted
- **Deciders:** Vinny Carpenter

## Context

Transcripts contain full message content, which can include secrets. The
privacy rules (spec §8) say content must never be parsed, displayed,
exported, or persisted. Separately, transcript schemas drift across Claude
Code versions, records of 15+ types share files, and malformed lines are
normal input.

## Decision

Deserialize every line into a single permissive `RawRecord` struct in which
**every field is `Option`** and **no field for message content exists**.
Unknown fields are ignored by serde default; unknown `type` values and
unparseable lines are skipped and counted, never fatal. A second, strict
type (`UsageEvent`) is constructed from `RawRecord` and is the only shape
the rest of the codebase sees.

## Consequences

- Easier: privacy review is `grep` — content leakage requires adding a field,
  which a diff makes obvious. Schema drift degrades gracefully into skip
  counts surfaced by `doctor` instead of crashes.
- Harder: serde still lexes over content bytes (unavoidable without manual
  scanning); performance work must come from streaming + parallelism, not
  schema tricks.
- Out of scope: validating fields we do not aggregate.

## Alternatives

- **Typed per-record-type enum** (`#[serde(tag = "type")]`): rejected — every
  new record type Claude Code ships becomes a parse failure instead of a
  silent skip.
- **Parse content but redact at display time:** rejected outright — policy
  enforcement at the edge of output is one bug away from leaking; absence at
  the type level cannot leak.
