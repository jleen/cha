#!/bin/sh
# Run the cha-core performance suite. See docs/core.md.
#
#   ./scripts/perf.sh --save      # on the "before" build
#   ./scripts/perf.sh --compare   # on the "after" build
#
# Two things this wrapper exists for: forcing --release (a debug build makes the
# matcher 10-50x slower and the numbers meaningless), and running from the repo
# root so a relative word list resolves the same way every time.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p cha-core --example perf
exec ./target/release/examples/perf "$@"
