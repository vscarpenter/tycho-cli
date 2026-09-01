# tycho — task state

## Ollama Cloud pricing (2026-08-31) — done

Prompted by Vinny sharing ollama.com's pricing table. Ollama Cloud is metered
per token, but every one of its ids carries a ':' and so fell under the
local-model rule that reports `name:tag` ids at $0. That hid 32.9M tokens of
`glm-5.3:cloud`.

- [x] `pricing/default.toml`: all 19 published rows plus their `<name>:cloud`
      twins (`68ba1e2`). The twin is load-bearing — lookup needs a '-'
      boundary, so the key `glm-5.3` does NOT match the id `glm-5.3:cloud`.
- [x] `auto` mode falls through a recorded cost of exactly zero (`4332134`).

**No heuristic changed.** The rule was already "a ':' id with *no pricing
entry* is local", so an explicit entry is sufficient and local tags keep
their correct $0. A regression test pins that `gemma4:12b` never resolves
onto the new `gemma4` stanza one '-' boundary away.

The second commit was the non-obvious half: pricing data alone left the
default report at $0, because Pi stamps `usage.cost.total = 0` on all 211 of
its Ollama records and `auto` preferred that over the table. Default-mode
effect: `glm-5.3:cloud` $0.00 -> $46.59; Bedrock Opus $3.62 unchanged
(non-zero, still preferred); local tags $0.00 unchanged (no entry).

### Resuming from here

- **`auto`'s contract changed for every provider**, not just Pi: a record
  carrying `costUSD: 0` on a priced model now gets table rates. README and
  `docs/SCHEMA.md` state it.
- **Three ids are knowingly ambiguous.** `gpt-oss:120b`, `gpt-oss:20b` and
  `qwen3.5:397b` are billed on Ollama Cloud but identical to a local pull.
  Vinny chose to price them as cloud; the local-override recipe is in
  `pricing/default.toml` at the point someone would need it. If local runs of
  those ever show up in the corpus, revisit.
- **Cache-write rates are 0, deliberately.** Ollama publishes no write rate;
  deriving one from the 1.25x/2x input multipliers would be a guess printed
  as a number. Revisit only if Ollama publishes one.

## Pi provider (2026-08-31) — done, unreleased

Spec `tasks/spec.md`; decision recorded in `docs/adr/0003-pi-provider.md`.
Pi's transcripts were invisible: not a default root, and the line shape
matched none of the three formats `Provider::External` sniffs. One machine's
sessions held 23M uncounted tokens, incl. $89 of `gpt-5.5-pro`.

- [x] 1 — `discover.rs`: `Provider::Pi`, `default_roots` gains `pi_agent_dir`,
      `project_name` returns the `(pi)` placeholder (`5a510eb`)
- [x] 2 — `record.rs`: Pi fields on `RawUsage`, `role` on `RawMessage`,
      `pi_token_usage()`, the `Provider::Pi` parse branch (`5a510eb`)
- [x] 3 — `cli.rs` + `main.rs`: `ProviderArg::Pi`, `PI_CODING_AGENT_DIR`
      (`5a510eb`)
- [x] 4 — `tests/cli.rs` integration + Pi fixture (`c8e4c81`)
- [x] 5 — `docs/SCHEMA.md` + README: Pi layout, records, cache-TTL caveat
      (`c8e4c81`)
- [x] 6 — verified: 210 tests, fmt + clippy(-D warnings) green at each commit

Ground truth: verified against the real `~/.pi/agent/sessions` corpus inside a
proven-stable read window (see `tasks/lessons.md`) — all six models match an
independent Python oracle exactly on input, output, cache write, cache read,
total, and cost. **36,755,778 tokens and $93.29 that tycho previously counted
at zero**, of which $89.67 is `gpt-5.5-pro` and $3.62 is Bedrock-routed Opus
priced only because Pi records its own cost.

### Resuming from here

- **Not released:** no version bump, no tag. Pi adds spend to historical
  totals, so call that out in release notes the way the Codex backfill was.
- **Deferred by design (spec "Out of scope"):** the External `--dir` sniffer
  still does not recognize the Pi envelope; the pricing table still cannot
  resolve Bedrock region prefixes (`us.` / `eu.` / `apac.`), so
  `--mode calculate` reports $0 for `us.anthropic.*` while the default `auto`
  mode reports Pi's own figure.
- **Meta Muse is NOT covered.** Its logs are at
  `~/.local/share/muse/sessions/<yyyy>/<mm>/<dd>/<uuid>/session.jsonl`, an
  event-sourced envelope with usage nested at `payload.event.record.quantity`
  under `payload_type: "runtime.session"`, no model id on the usage record (it
  comes from separate `run.model.configured` events), and microsecond-epoch
  `recorded_at` that `bounded_epoch` rejects. Needs its own design.
- **Resolved, not a pending bump:** `pricing/default.toml` used to say Sonnet 5
  intro pricing ended 2026-08-31 and should rise to $3/$15. Anthropic made the
  $2/$10 rate **permanent** on 2026-08-11 and cancelled the increase, verified
  2026-08-31 against the live docs. tycho's rates were already right; only the
  misleading comment changed. Do not "fix" those rates to $3/$15.

## Phase plan (spec §10)

- [x] Phase 0 — recon, `docs/SCHEMA.md`, plan sign-off (2026-07-04)
- [x] Phase 1 — parse & report: discovery, streaming parser, dedupe,
      aggregation, `tycho daily` with table + `--json`, fixtures, tests
- [x] Phase 2 — breadth: monthly/sessions/projects/models/doctor, `--csv`,
      rayon, benchmark script (2026-07-04)
- [x] Phase 3 — money: pricing engine, cost modes, rust_decimal, `cache`
      report (2026-07-04)
- [x] Phase 4 — live: ratatui dashboard (2026-07-04)
- [x] Phase 5 — ship (stretch): A billing-blocks, B cargo-dist, C Homebrew
      tap, D vhs demo — all done (2026-07-04)
  - [x] 5A — `tycho blocks` (2026-07-04)
  - [x] licensing — MIT (LICENSE + Cargo.toml, 2026-07-04)
  - [x] 5B — cargo-dist release automation + install docs (2026-07-04)
  - [x] 5C — Homebrew tap (folded into 5B; repo + formula config done)
  - [x] 5D — vhs demo (demo.tape + demo.gif, embedded in README, 2026-07-04)

## Phase 5B review (at gate, 2026-07-04)

- cargo-dist 0.32.0 wired: `dist-workspace.toml` (5 targets, shell/powershell/
  homebrew installers, tap = vscarpenter/homebrew-tap, formula = tycho),
  generated `.github/workflows/release.yml`, `[profile.dist]` +
  repository/homepage in Cargo.toml.
- `dist plan` resolves all five targets + three installers clean; fmt/clippy/
  release/tests still green. README Install section rewritten (cargo/source
  works today; brew/shell/ps installers ship with each tagged release).
- Public tap repo created: github.com/vscarpenter/homebrew-tap.
- Boundary held: NO release cut. release.yml publishes only on a `v*` tag.
- REMAINING for a real release (needs Vinny): add a `HOMEBREW_TAP_TOKEN`
  repo secret (PAT with write to homebrew-tap), bump version if desired, then
  `git tag vX.Y.Z && git push --tags`.
- Next: 5D (vhs demo) — its own design gate.

## Phase 5A review (at gate, 2026-07-04)

- `tycho blocks`: activity-anchored 5-hour billing windows (ccusage-style),
  built as a stateful fold over sorted events (not a group-by); the block
  containing an injected `now` carries a linear cost projection (Decimal) plus
  token/burn estimates (f64). `--json` contract; `--csv` rejected.
- 8 blocks tests (6 unit + 2 integration); 139 tests total, all TDD;
  fmt/clippy(-D warnings) green at every commit. Verified the real table
  against fixtures (4 windows, total 1,510).
- Public repo live at github.com/vscarpenter/tycho-cli; CI green on
  ubuntu/macos + release + windows (actions/checkout@v5).
- Design/plan under `docs/superpowers/{specs,plans}/2026-07-04-billing-blocks*`;
  learning note `docs/learning/phase-5.md`.
- Next: 5B (cargo-dist) — needs its own design gate.

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

## Cost truthfulness (2026-08-09) — done, unreleased

Prompted by a tycho-vs-ccusage reconciliation. tycho was correct everywhere
the two disagreed (Codex coverage, streaming-dedup tie-break, 1h cache
pricing); the investigation surfaced three places tycho itself lost data it
had already parsed. Spec + plan under
`docs/superpowers/{specs,plans}/2026-08-09-cost-truthfulness*`.

- [x] Codex model backfill (`30818c8`) — token_count before turn_context kept
      the zero-rated `codex-unknown` placeholder. Backfills from the file's
      first turn_context; counted as "Codex models backfilled" in doctor.
      Real corpus: 3,360 events, bucket 434,311,580 → 27,380,832 tokens,
      **+$311.28** previously priced at $0. Historical totals now increase.
- [x] Cache TTL split (`8c6c855`) — `cache` gains Write 5m / Write 1h columns
      and a premium sentence. Real corpus: 72.8% of writes use the 1h TTL,
      costing $335.02 over the 5m rate. Added `cache_contract` JSON test, which
      the module had never had.
- [x] Shadow estimates (`bb7b036`) — `[shadow]` section maps zero-rated ids to
      a priced reference; doctor reports unpriced tokens + estimate. Real
      corpus: 419,779,330 tokens across 6 models, $176.64. Diagnostic only.
- [x] `4b9eefa` is unrelated rustfmt 1.9.0 drift that already failed at HEAD,
      split out so the feature commits stay clean.

191 tests, fmt + clippy(-D warnings) green at every commit. Not released:
no version bump, no tag.

### Resuming from here

- **Next if releasing:** bump version, tag `vX.Y.Z`. The backfill changes
  historical totals, so call that out in the release notes — README accuracy
  caveats already warn about it.
- **Deferred by design (see spec "Out of scope"):** a global `--shadow` flag
  exposing the estimate on daily/monthly/models; a `tycho audit` command;
  broader local-LLM support (already handled — 0.011% of the corpus and
  genuinely free).
- **Open question worth revisiting:** `[shadow]` maps the legacy codex labels
  to `gpt-5.4` as the same-generation priced model. That choice is a judgment
  call, not a fact; revisit if those labels turn out to bill differently.
- **Unchanged pre-existing item:** `rust-toolchain.toml` pins `channel =
  "stable"` rather than a version, which is what let rustfmt drift in. Pinning
  it would prevent a repeat.

## Resuming from here (v1, superseded by the section above)

- v1 definition of done (§13) was met at the Phase 3 gate; Phase 4 (live)
  is complete pending Vinny's gate review.
- Next: Phase 5 stretch (vhs demo, LICENSE files, cargo-dist, Homebrew tap,
  billing-blocks report).
- CI has never run remotely (no GitHub remote configured yet) — push and
  verify before calling CI green.
- ~~pricing/default.toml notes Sonnet 5 intro pricing ends 2026-08-31 —
  bump to $3/$15 after that date.~~ Superseded 2026-08-31: Anthropic made
  $2/$10 permanent on 2026-08-11. No bump; rates stay as they are.
