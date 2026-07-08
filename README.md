<p align="center">
  <img src="assets/tycho_banner_1280x640.png" alt="tycho — usage analytics for local AI tool transcripts: tokens, cost, cache economics, and trends" width="760">
</p>

# <img src="assets/tycho_icon_512.png" alt="" height="28"> tycho

Fast, privacy-first usage analytics for local AI tool transcripts and response
logs: tokens, cost, cache economics, and trends by day, project, session, and
model. It scans [Claude Code](https://code.claude.com), Codex, and readable
OpenAI JSONL usage records. Think `iostat` for AI spend.

![tycho demo: the daily, cache, and blocks reports](demo.gif)

```
$ tycho cache
Your effective cost was $3741.36. Without prompt caching it would have been $19510.97.
Caching saved you $15769.62 (5.2x leverage).

┌───────────────────┬───────────────┬─────────────┬──────────┬─────────────┬───────────────┬───────────┬──────────┐
│ Model             ┆ Cache Read    ┆ Cache Write ┆ Hit Rate ┆ Actual Cost ┆ No-Cache Cost ┆ Savings   ┆ Leverage │
╞═══════════════════╪═══════════════╪═════════════╪══════════╪═════════════╪═══════════════╪═══════════╪══════════╡
│ claude-opus-4-8   ┆ 1,882,805,783 ┆ 66,040,314  ┆ 96.4%    ┆ $1966.10    ┆ $10143.74     ┆ $8177.65  ┆ 5.2x     │
│ claude-fable-5    ┆ 749,241,182   ┆ 30,597,036  ┆ 95.7%    ┆ $1589.18    ┆ $8134.27      ┆ $6545.09  ┆ 5.1x     │
│ claude-sonnet-5   ┆ 518,361,906   ┆ 6,099,019   ┆ 98.7%    ┆ $135.57     ┆ $1061.00      ┆ $925.44   ┆ 7.8x     │
│ Total             ┆ 3,207,875,363 ┆ 110,796,131 ┆ 96.4%    ┆ $3741.36    ┆ $19510.97     ┆ $15769.62 ┆ 5.2x     │
└───────────────────┴───────────────┴─────────────┴──────────┴─────────────┴───────────────┴───────────┴──────────┘
```

## Install

The package is `tycho-cli`; the installed binary is `tycho`.

**From source** (works today; requires Rust 1.88+ stable):

```sh
cargo install --git https://github.com/vscarpenter/tycho-cli
```

**Prebuilt binaries and installers** (available with each tagged release):

```sh
# Homebrew (macOS/Linux)
brew install vscarpenter/tap/tycho

# Shell installer (macOS/Linux)
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/vscarpenter/tycho-cli/releases/latest/download/tycho-cli-installer.sh | sh

# PowerShell installer (Windows)
powershell -c "irm https://github.com/vscarpenter/tycho-cli/releases/latest/download/tycho-cli-installer.ps1 | iex"
```

Prebuilt archives for macOS (arm64/x86_64), Linux (arm64/x86_64), and Windows
(x86_64) are attached to each [GitHub Release](https://github.com/vscarpenter/tycho-cli/releases).

## Quickstart

```sh
tycho                 # daily token + cost table (the default command)
tycho cache           # what prompt caching is saving you
tycho sessions --limit 10 --sort tokens
tycho projects
tycho models
tycho monthly --csv > usage.csv
tycho doctor          # data-health: files, skipped lines, duplicates, span
tycho live            # a live dashboard that refreshes every 2 seconds
```

```
$ tycho daily --since 2026-07-01
┌────────────┬─────────┬─────────┬─────────────┬─────────────┬──────────────┬────────────┐
│ Date       ┆ Input   ┆ Output  ┆ Cache Write ┆ Cache Read  ┆ Total Tokens ┆ Cost (USD) │
╞════════════╪═════════╪═════════╪═════════════╪═════════════╪══════════════╪════════════╡
│ 2026-07-01 ┆ 57,502  ┆ 236,565 ┆ 1,043,016   ┆ 33,839,653  ┆ 35,176,736   ┆ $47.31     │
│ 2026-07-02 ┆ 422,681 ┆ 990,627 ┆ 6,125,572   ┆ 199,047,245 ┆ 206,586,125  ┆ $221.98    │
│ 2026-07-03 ┆ 290,698 ┆ 760,432 ┆ 3,690,105   ┆ 464,019,926 ┆ 468,761,161  ┆ $295.94    │
│ Total      ┆ 770,881 ┆ 1,987,… ┆ 10,858,693  ┆ 696,906,824 ┆ 710,524,022  ┆ $565.23    │
└────────────┴─────────┴─────────┴─────────────┴─────────────┴──────────────┴────────────┘
```

## Commands

| Command | Purpose |
|---|---|
| `tycho daily` | Per-day tokens and cost (default when run bare) |
| `tycho monthly` | Same shape, monthly buckets |
| `tycho sessions` | Per-session: start, duration, project, models, tokens, cost. `--limit N`, `--sort start\|tokens\|duration` |
| `tycho projects` | Rollup by project directory |
| `tycho models` | Rollup by model |
| `tycho cache` | Cache economics: hit rate, actual vs no-cache cost, savings, leverage |
| `tycho doctor` | Data health: files, skipped lines, duplicates collapsed, date span, unpriced models |
| `tycho live` | Live dashboard: today's totals, 10-minute burn rate, per-model split, cache gauge, active project |
| `tycho blocks` | Per 5-hour billing block: tokens, cost, models, and the active block's projected total |

### `tycho blocks`

Groups usage into **activity-anchored 5-hour billing blocks** that mirror how
Claude's usage limits reset: a block starts at your first message (floored to
the hour) and spans five hours; a new block begins at the next message once
that window closes. For the block containing "now", tycho projects where its
cost lands if the current rate holds — `$2.05 so far, ~$4.10 projected by
17:00`. Since the 5-hour reset is a Claude-specific mechanic, `blocks`
defaults to Claude events only; pass `--provider codex` or `--provider openai`
to switch it to another provider's events, or `--provider all` to include
every provider. Honors the usual filters and `--json`; `--csv` is not
supported.

```
$ tycho blocks
Active block: $2.05 so far, ~$4.10 projected by 17:00 (612 tok/min).

Block (5h)          │ Status              │ Total Tokens │ Cost (USD)
07-04 09:00 – 14:00 │ 5h                  │ 12,431,002   │ $8.10
07-04 14:00 – 19:00 │ ● active · 2h30m left │ 3,120,540  │ $2.05 → ~$4.10
```

### `tycho live`

A [ratatui](https://ratatui.rs) dashboard that re-scans every 2 seconds on a
background thread, so the UI stays responsive. It shows today's totals and
cost, a tokens-per-minute burn rate over the last 10 minutes (with a
sparkline), a cache hit-rate gauge, the per-model split, today's sessions, and
the active project — detected from the most recently written transcript file.
Keys: `q` (or `Esc`/`Ctrl-C`) quits, `Tab` cycles the Overview / Sessions /
Models views. It ignores `--since`/`--until` (it is always "now") but honors
`--dir`, `--project`, `--model`, `--mode`, `--pricing`, and `--tz`/`--utc`.

`tycho live --json` — or `live` with a piped/redirected (non-TTY) stdout —
prints a single JSON snapshot of the dashboard state and exits, so it never
corrupts a pipe and can feed a status bar or script.

Global flags on every command: `--dir <PATH>` (repeatable; replaces default
search roots), `--since`/`--until` (inclusive dates in the report timezone),
`--project <SUBSTR>`, `--model <SUBSTR>`, `--provider claude|codex|openai|all`
(filters to one provider's events, or `all` for every provider; `blocks`
defaults to `claude` and this flag overrides it), `--tz <IANA>`/`--utc`,
`--mode auto|calculate|display`,
`--pricing <PATH>`, `--precise`, `--json`. `--csv` works on `daily`,
`monthly`, and `sessions`.

`--json` output is a stable, documented contract — treat it as the scripting
API. Fields are added over time but never renamed or removed.

## Where the data comes from

By default, `tycho` scans these read-only roots when they exist, each tagged
with the provider that owns its layout:

- Claude Code JSONL transcripts under `~/.claude/projects/`, or the
  directories in `CLAUDE_CONFIG_DIR` (Claude).
- Xcode's `CodingAssistant` Claude location on macOS (Claude).
- Codex JSONL session logs under `$CODEX_HOME/sessions` or
  `~/.codex/sessions` (Codex).

Claude roots parse only Claude's `assistant` records; Codex roots parse only
Codex's token-count records. `--dir <PATH>` (repeatable) replaces the default
roots entirely with your own directories, tagged `External`; those are
format-sniffed line by line — tycho tries the Claude shape, then Codex's, then
falls back to a standalone OpenAI Responses API or Chat Completions record,
gated only on a top-level `usage` object being present. Because that gate
doesn't check for OpenAI-specific fields, a `--dir` directory should contain
only usage logs: any foreign JSONL line that happens to carry a `usage` object
is counted as an event. The current macOS ChatGPT desktop conversation cache
is opaque `.data` storage rather than plain JSONL usage records, so tycho does
not scan that cache by default — point `--dir` at an actual export/log
directory instead.

`tycho` streams each file, collapses duplicate streaming records, and
aggregates. A full scan of 500 MB takes well under a second on an Apple
M-series machine.

## Pricing

A default pricing table (USD per million tokens, including Claude cache-write
TTL splits and cached-input rates for supported OpenAI models) ships inside
the binary; its sources and verification date are recorded in
[`pricing/default.toml`](pricing/default.toml). Override any model — or add
new ones — at `~/.config/tycho/pricing.toml` (or `$XDG_CONFIG_HOME/tycho/`),
or per run with `--pricing <PATH>`. Models with no pricing entry cost $0,
warn once on stderr, and are listed by `doctor`.

Cost modes mirror ccusage: `--mode auto` (default) uses a record's
pre-computed `costUSD` when present and calculates otherwise; `calculate`
always prices from tokens; `display` only sums recorded `costUSD` values
(modern Claude Code and Codex records don't write `costUSD`, so `display` is
only meaningful for old transcripts or custom logs that include recorded cost).

## Accuracy caveats

- `output_tokens` in transcripts can be a mid-stream snapshot rather than
  the final count, so output tokens may be undercounted
  ([claude-code#27361](https://github.com/anthropics/claude-code/issues/27361)).
  Numbers are estimates for trend analysis, **not an invoice reconciliation
  tool**.
- Claude Code prunes old transcripts based on its `cleanupPeriodDays`
  setting, and Codex/ChatGPT local retention can also change, so history has a
  horizon. `tycho doctor` prints the observed date span.
- OpenAI subscription-plan usage is not the same as API invoicing. Built-in
  OpenAI prices estimate API-equivalent token cost; override pricing for long
  context, Batch, Flex, Priority, data residency, or workspace-specific rates.
- Historical/private Codex labels with no public API rate are explicitly
  priced at $0 to avoid noisy warnings; `doctor` lists them separately as
  "Zero-rated models" rather than mixing them in with genuinely unpriced
  models. Override them at `~/.config/tycho/pricing.toml` if your account
  bills those labels differently.
- Ollama-style local model ids (`name:tag`) carry no pricing entry at all;
  `doctor` lists them separately as "Local models" instead of warning about
  them every run.

## Privacy

1. **Read-only.** tycho never writes inside any supported tool config directory.
2. **No network calls at runtime.** Pricing is embedded or read from local
   config. No telemetry, ever.
3. **Message content is never parsed, displayed, exported, or persisted.**
   The deserialization types capture only the metadata envelope (ids,
   timestamps, model, token counts) — there is no field that could hold
   content, so leaking it is structurally impossible. Transcripts can
   contain secrets; this design keeps them out of tycho entirely.

## Prior art

[ccusage](https://github.com/ryoppippi/ccusage) (TypeScript) proved the
demand for this kind of tool and defined the cost-mode semantics tycho
mirrors. tycho is the Rust-native take with a stronger cache-economics
story.

## License

[MIT](LICENSE) © Vinny Carpenter.
