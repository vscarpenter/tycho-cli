#!/usr/bin/env bash
# Honest benchmark: generate a synthetic corpus of the requested size and
# time a full `tycho daily` scan over it.
#
# Usage: scripts/bench.sh [MB] [DIR]
#   MB   corpus size to generate (default 500)
#   DIR  where to put it (default: a fresh temp dir; reused if it exists)
set -euo pipefail

MB="${1:-500}"
DIR="${2:-${TMPDIR:-/tmp}/tycho-bench-${MB}mb}"

cargo build --release --quiet
cargo build --release --quiet --example generate_fixtures

if [ ! -d "$DIR" ]; then
  echo "generating ${MB} MB corpus in $DIR ..."
  ./target/release/examples/generate_fixtures "$DIR" "$MB"
else
  echo "reusing corpus in $DIR"
fi

echo "corpus size: $(du -sh "$DIR" | cut -f1), files: $(find "$DIR" -name '*.jsonl' | wc -l | tr -d ' ')"
echo "--- tycho daily (full scan) ---"
time ./target/release/tycho daily --utc --dir "$DIR" > /dev/null
