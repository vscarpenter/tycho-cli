# tycho

Fast, privacy-first usage analytics for [Claude Code](https://code.claude.com)'s
local JSONL transcripts: tokens, cost, cache economics, and trends by day,
project, session, and model. Think `iostat` for AI spend.

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

Requires Rust 1.88+ (stable).

```sh
git clone https://github.com/vscarpenter/tycho-cli
cd tycho-cli
cargo install --path .
```

The package is `tycho-cli`; the installed binary is `tycho`.

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
`--project <SUBSTR>`, `--model <SUBSTR>`, `--tz <IANA>`/`--utc`,
`--mode auto|calculate|display`, `--pricing <PATH>`, `--precise`, `--json`.
`--csv` works on `daily`, `monthly`, and `sessions`.

`--json` output is a stable, documented contract — treat it as the scripting
API. Fields are added over time but never renamed or removed.

## Where the data comes from

Claude Code writes one JSONL transcript per session under
`~/.claude/projects/` (or the directories in `CLAUDE_CONFIG_DIR`, plus
Xcode's `CodingAssistant` location when present). `tycho` scans them
read-only, streams each file, collapses duplicate streaming records, and
aggregates. A full scan of 500 MB takes well under a second on an Apple
M-series machine.

## Pricing

A default pricing table (USD per million tokens, including the 5-minute and
1-hour cache-write TTL split and cache reads) ships inside the binary; its
sources and verification date are recorded in
[`pricing/default.toml`](pricing/default.toml). Override any model — or add
new ones — at `~/.config/tycho/pricing.toml` (or `$XDG_CONFIG_HOME/tycho/`),
or per run with `--pricing <PATH>`. Models with no pricing entry cost $0,
warn once on stderr, and are listed by `doctor`.

Cost modes mirror ccusage: `--mode auto` (default) uses a record's
pre-computed `costUSD` when present and calculates otherwise; `calculate`
always prices from tokens; `display` only sums recorded `costUSD` values
(modern Claude Code versions don't write `costUSD`, so `display` is only
meaningful for old transcripts).

## Accuracy caveats

- `output_tokens` in transcripts can be a mid-stream snapshot rather than
  the final count, so output tokens may be undercounted
  ([claude-code#27361](https://github.com/anthropics/claude-code/issues/27361)).
  Numbers are estimates for trend analysis, **not an invoice reconciliation
  tool**.
- Claude Code prunes old transcripts based on its `cleanupPeriodDays`
  setting, so history has a horizon. `tycho doctor` prints the observed
  date span.

## Privacy

1. **Read-only.** tycho never writes inside any Claude config directory.
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
