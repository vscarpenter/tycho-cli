# tycho — task state

## Phase plan (spec §10)

- [x] Phase 0 — recon, `docs/SCHEMA.md`, plan sign-off (2026-07-04)
- [x] Phase 1 — parse & report: discovery, streaming parser, dedupe,
      aggregation, `tycho daily` with table + `--json`, fixtures, tests
- [x] Phase 2 — breadth: monthly/sessions/projects/models/doctor, `--csv`,
      rayon, benchmark script (2026-07-04)
- [x] Phase 3 — money: pricing engine, cost modes, rust_decimal, `cache`
      report (2026-07-04)
- [x] Phase 4 — live: ratatui dashboard (2026-07-04)
- [ ] Phase 5 — ship (stretch): vhs demo, LICENSE files, cargo-dist,
      Homebrew tap, billing-blocks report

## Phase 2 + 3 review (at gate, 2026-07-04)

- 110 tests (90 unit + 20 integration), all TDD; fmt/clippy(-D warnings)
  green at every commit.
- Ground truth vs independent jq oracles on the real 549 MB corpus (frozen
  ≤2026-07-03 window): models/monthly/sessions token totals exact;
  calculate-mode cost exact to the limit of f64 (oracle 3420.6745642999927
  vs tycho's exact 3420.6745643).
- Benchmark: 502 MB synthetic corpus scans in 0.30 s; real corpus 0.09 s
  (target < 3 s).
- Crate-list amendments to flag: chrono-tz + iana-time-zone (P1, tz),
  rayon/csv/rust_decimal (pre-approved), toml (parser for the spec's chosen
  format). `directories` was NOT needed (env + home cover the spec paths).
- README shipped with §13 content: install, quickstart, real sample output,
  accuracy caveats, privacy statement, ccusage credit.

## Phase 4 review (at gate, 2026-07-04)

- `tycho live`: ratatui dashboard, 2 s refresh on a background scan thread
  (mpsc snapshots), three tabs (Overview/Sessions/Models), 10-min burn
  sparkline, cache gauge, mtime-based active-session detection. `q`/`Tab`
  keys; `--json`/non-TTY prints one snapshot and exits.
- 129 tests (108 unit + 21 integration), all TDD; fmt/clippy(-D warnings)
  green at every commit. Pure `DashboardState::derive` (injected `now`) is
  unit-tested; views verified against a ratatui `TestBackend`; the JSON
  snapshot path has an end-to-end CLI test.
- Verified in a real PTY: enters/leaves the alternate screen, runs the
  redraw loop with live data, quits cleanly on `q` (exit 0), restores the
  terminal.
- New crate: `ratatui` (pre-approved P4); crossterm consumed via
  `ratatui::crossterm` re-export. `scan::scan_files` seam added so live
  discovers once and reuses the list for both mtimes and parsing.
- Design/plan under `docs/superpowers/{specs,plans}/2026-07-04-live-*`.

## Resuming from here

- v1 definition of done (§13) was met at the Phase 3 gate; Phase 4 (live)
  is complete pending Vinny's gate review.
- Next: Phase 5 stretch (vhs demo, LICENSE files, cargo-dist, Homebrew tap,
  billing-blocks report).
- CI has never run remotely (no GitHub remote configured yet) — push and
  verify before calling CI green.
- pricing/default.toml notes Sonnet 5 intro pricing ends 2026-08-31 —
  bump to $3/$15 after that date.
