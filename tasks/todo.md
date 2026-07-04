# tycho — task state

## Phase plan (spec §10)

- [x] Phase 0 — recon, `docs/SCHEMA.md`, plan sign-off (2026-07-04)
- [x] Phase 1 — parse & report: discovery, streaming parser, dedupe,
      aggregation, `tycho daily` with table + `--json`, fixtures, tests
- [ ] Phase 2 — breadth: monthly/sessions/projects/models/doctor, `--csv`,
      rayon, benchmark script
- [ ] Phase 3 — money: pricing engine, cost modes, rust_decimal, `cache` report
- [ ] Phase 4 — live: ratatui dashboard
- [ ] Phase 5 — ship (stretch): README polish, licensing, cargo-dist

## Phase 1 review (done at gate, 2026-07-04)

Built: `discover` → `record` → `dedupe` → `aggregate` → `report::{table,json}`
→ `scan` → `cli`/`main`. 48 unit + 7 integration tests, all TDD (red
watched before green in every module; clap grammar treated as declarative
config with pinning tests). fmt/clippy(-D warnings)/tests green.

Verified against reality:
- All 33 closed days match an independent jq oracle **exactly** on all five
  token categories; the only differing day was today, because the corpus
  grows while being scanned (confirmed by rescanning).
- Full 549 MB / 1,004-file scan: **1.63 s** wall, single-threaded
  (target < 3 s; rayon still pending for Phase 2).
- Privacy: no content-capable field in any serde type; only test code
  writes files.

Spec deviations (flagged at gate): tokens-only Phase 1 daily (user-approved),
`chrono-tz` + `iana-time-zone` crates added, MSRV 1.88 (let-chains).

## Resuming from here

- Next: Phase 2 (monthly, sessions, projects, models, doctor, --csv, rayon,
  fixture-generator benchmark script) after Vinny reviews the Phase 1 gate
  summary.
- `ScanSummary`/`ParseStats` already collect everything doctor needs;
  doctor is mostly a renderer.
- Sessions rollup: `UsageEvent.session_id` carries the parent session for
  subagent files (verified in recon); duration = last − first timestamp.
- Watch: `report::json` contract is documented in doc comments; add
  `docs/json-schema.md` when the surface grows in Phase 2.
