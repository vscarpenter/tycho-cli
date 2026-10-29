# Lessons

- **Never verify through a pipe without `pipefail`.** `cargo clippy ... | tail -1`
  masked a clippy failure (pipeline exit code comes from `tail`), letting a
  bad commit land; it was caught one step later and amended. Verification
  commands must either run bare or use `set -o pipefail`.

- **Verify against a live data source inside a proven-stable window.** Checking
  the Pi provider against `~/.pi/agent/sessions` showed a 218,385-token gap on
  one model while the other five matched exactly. Nothing was wrong: Pi was
  appending to that day's session file between the tycho run and the oracle
  run. Read the oracle, run tycho, read the oracle again, and compare only when
  the two oracle reads are identical — otherwise you are debugging a phantom.
  The "one model off, the rest exact" shape is the tell.

- **Re-verify every stanza in a pricing round, not only the new releases.**
  Anthropic cut Claude Sonnet 5.5 cache reads from $0.20 to $0.10 nine days
  after launch. The release tracker lists launches, not price changes, so
  only re-reading the whole Anthropic table caught it.

- **Probe Pi-shaped events through Pi discovery, not `--dir`.** `--dir`
  parses every file as a Claude or External transcript, so a Pi probe
  reports zero events. Run the binary with `HOME` set to an empty temp
  directory and `PI_CODING_AGENT_DIR` set to a folder holding
  `sessions/<project>/<file>.jsonl`, then pass `--provider pi --mode
  calculate`. Set the Pi `cost` fields to zero as well, so even the default
  auto mode prices from the table.
