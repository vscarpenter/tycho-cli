# Lessons

- **Never verify through a pipe without `pipefail`.** `cargo clippy ... | tail -1`
  masked a clippy failure (pipeline exit code comes from `tail`), letting a
  bad commit land; it was caught one step later and amended. Verification
  commands must either run bare or use `set -o pipefail`.
