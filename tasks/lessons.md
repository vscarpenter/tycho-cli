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
