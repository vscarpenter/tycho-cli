# Implementation Prompt: ccstat

**Working name:** `ccstat` (in the spirit of `iostat` and `vmstat`). Alternates I may switch to: `tycho`, `ledger`. Keep the binary name defined in one place so renaming is a one-line change.

---

## 1. Mission

Build a fast, privacy-first Rust CLI that turns Claude Code's local JSONL transcripts into usage analytics: tokens, cost, cache economics, and trends by day, project, session, and model. Think `iostat` for AI spend.

Two goals, equally weighted:

1. **A real open-source tool.** Useful on day one, credible in a public repo, fast enough that nobody asks for a database.
2. **My Rust learning vehicle.** I am an experienced engineer (Go, Swift, TypeScript, Java) but new to Rust. How we build matters as much as what we build. See Section 9.

Prior art: `ccusage` (TypeScript, https://github.com/ryoppippi/ccusage) proved the demand. We are building the Rust-native take with a stronger cache-economics story and a teaching-quality codebase. Credit ccusage in the README as prior art.

---

## 2. How I want you to work

1. **Phased delivery.** Follow the phase plan in Section 10. At the end of each phase: stop, summarize what you built, what you learned about the data, and any spec deviations, then wait for my review.
2. **Plan before code.** Begin Phase 0 in plan mode and get my sign-off on the plan.
3. **Never invent schema.** Every field you parse must be observed in real transcript data during Phase 0, or documented in the references (Section 11). If the spec and reality disagree, reality wins: update `docs/SCHEMA.md` and flag it to me.
4. **Ask, don't assume**, when a product decision is ambiguous. Deviating from this spec without asking is a bug.
5. **Small, conventional commits.** One logical change per commit, conventional commit messages.
6. **No `unwrap()` or `expect()` outside tests.** Malformed input is normal input for this tool.
7. **Boring, idiomatic Rust beats clever Rust.** Every time.

---

## 3. Data source

### 3.1 Where the data lives

Claude Code writes one JSONL transcript file per session under the projects directory:

- Default root: `~/.claude/projects/<encoded-project-path>/**/*.jsonl`
- If `CLAUDE_CONFIG_DIR` is set, `~/.claude` is replaced by that directory. Treat the env var as a comma-separated list of roots.
- Windows: `%USERPROFILE%\.claude` (best-effort support, see Section 6).
- macOS bonus location (scan if present): `~/Library/Developer/Xcode/CodingAssistant/ClaudeAgentConfig/projects/` (Xcode's Claude integration writes here).
- CLI override: repeatable `--dir <PATH>` flag that replaces the default search roots.

Discover files with a recursive glob for `*.jsonl` under each root. Do not hardcode one directory nesting depth; layouts have varied across Claude Code versions.

### 3.2 Record shape (verify in Phase 0)

Each line is one JSON object. The records we care about are `type: "assistant"` entries, which carry approximately:

```json
{
  "type": "assistant",
  "uuid": "...",
  "timestamp": "2026-07-04T14:31:22.123Z",
  "sessionId": "...",
  "requestId": "...",
  "cwd": "/Users/vinny/projects/gsd",
  "version": "2.x.x",
  "message": {
    "id": "msg_...",
    "model": "claude-opus-4-...",
    "usage": {
      "input_tokens": 137,
      "cache_creation_input_tokens": 5521,
      "cache_read_input_tokens": 815193,
      "output_tokens": 4260
    }
  }
}
```

Some records include a pre-computed `costUSD` field (older Claude Code versions); most do not. Some newer records may include a cache-creation TTL breakdown (5-minute vs 1-hour cache writes). Phase 0 confirms all of this against my real data.

### 3.3 Parsing rules

- Stream line by line with a `BufReader`. Never load whole files into memory.
- Deserialize into permissive structs: unknown fields ignored, missing optional fields tolerated, unrecognized `type` values skipped silently.
- A malformed line is skipped and counted, never fatal. `doctor` reports the count.
- **Never retain message content.** Define serde types that only capture the metadata envelope (timestamps, ids, model, usage, cwd, version). Content fields must not exist in our types. Parse the envelope, ignore the letter.

### 3.4 Deduplication (this is load-bearing)

Streaming causes Claude Code to write multiple partial records for the same API message, with `usage` values that are cumulative snapshots. Naive summing double-counts badly.

- Identity key: `(message.id, requestId)` when both are present; fall back to `uuid`.
- For each identity key, keep exactly one record: the one with the greatest `output_tokens` (tie-break on latest timestamp). Do not sum records that share a key.
- `doctor` reports how many duplicates were collapsed.

### 3.5 Known accuracy caveats (document these in the README)

- `output_tokens` in transcripts can be a mid-stream snapshot rather than the final count, so output tokens may be undercounted. See https://github.com/anthropics/claude-code/issues/27361. Our numbers are estimates for trend analysis, not an invoice reconciliation tool.
- Claude Code prunes old transcripts based on its `cleanupPeriodDays` setting, so history has a horizon. `doctor` should print the observed date span.

---

## 4. Cost model

### 4.1 Pricing table

- Ship embedded default pricing via `include_str!` from `pricing/default.toml`.
- Per-model entries, USD per million tokens: `input`, `output`, `cache_write_5m`, `cache_write_1h`, `cache_read`.
- During development, verify current rates against the official pricing page (Section 11) and record the source URL and date in the TOML header.
- User override file at `$XDG_CONFIG_HOME/ccstat/pricing.toml` (macOS: also accept `~/.config/ccstat/pricing.toml`), plus a `--pricing <PATH>` flag. Override merges over defaults per model.
- Unknown model: warn once to stderr, price at zero, list in `doctor`. Never crash on a new model name.
- Support simple prefix matching so `claude-opus-4-20260115` matches a `claude-opus-4` entry.

### 4.2 Cost math

- Per record: `cost = input*p_in + output*p_out + cache_creation*p_write + cache_read*p_read`.
- If a TTL breakdown for cache writes is observed in Phase 0, price 5m and 1h writes separately; otherwise assume the 5-minute rate and say so in `docs/SCHEMA.md`.
- Use `rust_decimal` for all currency math. Floats are for graphs, not ledgers.
- Cost modes, mirroring ccusage semantics so users feel at home: `--mode auto` (use `costUSD` when a record has it, else calculate; default), `calculate` (always compute from tokens), `display` (only sum recorded `costUSD`).

### 4.3 Cache economics (the flagship feature)

For any aggregation window, compute and display:

- `cache_hit_rate = cache_read / (input + cache_creation + cache_read)`, guarded against divide-by-zero.
- `counterfactual_cost` = what the window would have cost with zero caching: `(input + cache_creation + cache_read) * p_in + output * p_out`.
- `savings = counterfactual_cost - actual_cost` and `leverage = counterfactual_cost / actual_cost`.

The `cache` report is the headline: "Your effective cost was X. Without prompt caching it would have been Y. Caching saved you Z (N.Nx leverage)." Per model and total.

---

## 5. Commands and UX

All commands share global flags: `--dir` (repeatable), `--since <DATE>`, `--until <DATE>`, `--project <SUBSTR>`, `--model <SUBSTR>`, `--mode`, `--json`, `--tz <IANA>` (default: system local), `--utc`.

| Command | Purpose |
|---|---|
| `ccstat daily` | Per-day table: input, output, cache write, cache read, total tokens, cost. Totals row. Default command when run bare. |
| `ccstat monthly` | Same shape, monthly buckets. |
| `ccstat sessions` | Per-session: start, duration, project, model(s), tokens, cost. `--limit N`, sortable. |
| `ccstat projects` | Rollup by project directory. |
| `ccstat models` | Rollup by model. |
| `ccstat cache` | The cache-economics report from 4.3. |
| `ccstat doctor` | Data health: roots scanned, files found, lines parsed and skipped, duplicates collapsed, records missing usage, unknown models, observed date span, bytes on disk. |
| `ccstat live` | Phase 4 ratatui dashboard, see 5.2. |

Output rules:

- Human output: clean tables via `comfy-table`, thousands separators on token counts, cost at 2 decimals (`--precise` for 4).
- `--json` on every command with a stable, documented schema (this is the scripting API; treat it as a contract).
- `--csv` on `daily`, `monthly`, and `sessions`.
- Respect `NO_COLOR`. Detect non-TTY and drop decoration automatically.
- Exit codes: 0 success, 1 runtime error, 2 usage error.

### 5.2 `ccstat live` (Phase 4)

A ratatui dashboard that refreshes every 2 seconds:

- Today's totals and cost, tokens-per-minute burn rate over the last 10 minutes, per-model split, and a cache hit rate gauge.
- Active session detection via recent file mtime; show the active project name.
- Keybindings: `q` quit, `tab` cycle views. Keep it simple; this is a monitor, not an app.

---

## 6. Technical constraints

- Rust stable, edition 2024, pinned via `rust-toolchain.toml` (channel `stable`). No nightly features. No `unsafe`.
- Single crate: `src/lib.rs` holds all logic (public API, so others can build on it), `src/main.rs` is a thin CLI shell.
- Suggested module layout: `discover`, `record` (serde types), `dedupe`, `pricing`, `cost`, `aggregate`, `report::{table,json,csv}`, `tui` (Phase 4).
- Crates (use judgment, justify any swaps in the phase summary): `clap` v4 with derive, `serde` + `serde_json`, `chrono` with serde feature, `thiserror` for library errors, `anyhow` in the binary only, `walkdir`, `rayon` (Phase 2), `comfy-table`, `rust_decimal`, `csv`, `directories`, `ratatui` + `crossterm` (Phase 4), `assert_cmd` + `predicates` (dev), `insta` (dev, optional).
- Platforms: macOS and Linux fully supported and tested in CI. Windows: compile in CI, best-effort behavior.
- Performance target: a full scan of 500 MB of transcripts completes in under 3 seconds on an Apple M-series machine, with flat memory (only aggregates retained). Include a fixture-generator script so we can benchmark honestly. If we cannot hit this with streaming plus rayon, propose an index as a Phase 5 discussion, do not build one preemptively.

---

## 7. Quality bar

- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean at every phase gate.
- Unit tests per module. Fixture JSONL files under `tests/fixtures/` must be **synthetic**; never commit real transcripts.
- Fixtures must cover: happy path, duplicate streaming records, malformed lines, missing usage, unknown record types, unknown models, `costUSD` present, and empty files.
- Integration tests for the CLI via `assert_cmd` (at minimum: `daily --json` end to end against fixtures).
- GitHub Actions CI: fmt, clippy, test on ubuntu-latest and macos-latest, plus a release build. Windows build job, tests allowed to skip.
- Library code returns typed errors (`thiserror`); only `main.rs` uses `anyhow`.

---

## 8. Privacy rules (non-negotiable)

1. Read-only. The tool never writes inside any Claude config directory.
2. No network calls at runtime. Pricing is embedded or read from local config. No telemetry, ever.
3. Message content is never parsed into our types, displayed, exported, or persisted. Metadata only.
4. State all three points in the README. Transcripts can contain secrets; our design must make leaking them structurally impossible.

---

## 9. Learning mode

I am using this project to learn Rust. Non-negotiables:

1. After each phase, write `docs/learning/phase-N.md` covering the 3 to 5 most instructive Rust concepts that phase exercised (ownership decisions, why a trait vs an enum, iterator patterns, error design), each with file and function pointers into the actual code.
2. When the borrow checker forces a design change, say so explicitly in the phase summary. Those moments are the curriculum.
3. The first time a lifetime annotation appears, stop and explain it before moving on.
4. Include one "Rust vs Go/Swift" contrast per phase, since those are my reference points.
5. Prefer iterators over index loops, and explain the choice once.
6. Doc comments (`///`) on every public item in the library.

---

## 10. Phase plan

**Phase 0, Recon (plan mode).** Read-only inspection of my real transcript directories. Sample old and new sessions. Produce `docs/SCHEMA.md` documenting every observed record shape, field, and anomaly, including whether `costUSD` and cache TTL breakdowns appear in my data. Propose the final data model and confirm the plan with me. No code yet.

**Phase 1, Parse and report.** Discovery, streaming parser, dedupe, aggregation, and `ccstat daily` with table and `--json` output. Fixtures and tests. This phase gate is the big one: numbers must be sane against a manual spot check.

**Phase 2, Breadth.** `monthly`, `sessions`, `projects`, `models`, `doctor`, all filters, `--csv`, rayon parallelism, benchmark script.

**Phase 3, Money.** Pricing engine, cost modes, `rust_decimal` throughout, and the `cache` report with counterfactual savings.

**Phase 4, Live.** The ratatui dashboard.

**Phase 5, Ship (stretch).** README polish with a demo recording (use `vhs`), dual MIT/Apache-2.0 licensing (Rust convention), `cargo install` instructions, GitHub release automation (`cargo-dist`), and optionally a Homebrew tap. Also on the table: a 5-hour billing-blocks report.

---

## 11. References (consult during Phase 0, do not guess)

- `.claude` directory layout and data files: https://code.claude.com/docs/en/claude-directory
- Claude Code docs map: https://docs.anthropic.com/en/docs/claude-code/claude_code_docs_map.md
- Output token undercount issue: https://github.com/anthropics/claude-code/issues/27361
- Current API pricing: https://claude.com/pricing
- Prior art to credit: https://github.com/ryoppippi/ccusage

---

## 12. Out of scope for v1

Codex or other tools' logs, multi-machine aggregation, any persistent database or index, a web UI, Claude Code for web sessions, and telemetry (out of scope forever).

---

## 13. Definition of done (v1)

Phases 0 through 3 complete. CI green (fmt, clippy with `-D warnings`, tests) on macOS and Linux. README covers install, quickstart with real-looking sample output, the accuracy caveats from 3.5, the privacy statement from Section 8, and prior-art credit. A stranger can clone the repo and get a `daily` report in under two minutes.
