# 0002 — Dedup by (message.id, requestId), keeping max output_tokens

- **Date:** 2026-07-04
- **Status:** Accepted
- **Deciders:** Vinny Carpenter

## Context

Streaming causes Claude Code to write multiple records for one API message
(observed worst case: 7). Their usage values are snapshots — identical in
observed data, but the format history suggests they can be cumulative
mid-stream values. Naive summing multiplies real usage several-fold.

## Decision

Identity is `(message.id, requestId)` when both are present, falling back to
`uuid`. Records are folded into a `HashMap<DedupKey, UsageEvent>` during the
scan; for each key exactly one record survives — the one with the greatest
`output_tokens`, tie-broken by latest timestamp. The number of collapsed
duplicates is counted and reported by `doctor`.

## Consequences

- Easier: correct totals regardless of whether duplicates are identical or
  ascending snapshots; a later, more complete record replaces an earlier one.
- Harder: memory is O(deduped records) for the scan window (~50k records ≈ a
  few MB), not O(1). Acceptable: the spec's "flat memory" goal is bounded by
  record count, not file size, and aggregation still never retains content.
- The map lives across files, so cross-file duplicates (if they ever occur)
  are also collapsed.

## Alternatives

- **First-wins `HashSet`** (ccusage's approach): rejected — cannot revise its
  choice when a later record carries the more complete usage snapshot.
- **Per-file dedup then merge:** rejected — saves negligible memory and
  silently assumes duplicates never span files.
