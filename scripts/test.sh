#!/bin/sh
# test.sh — run the full vmctl test suite across both workspaces.
#
# Tests live in the member crates, so a bare `cargo test` at the root only
# runs the workspace-level suite. This runs each crate's tests and reports a
# combined pass/fail, so one command covers the whole project.
#
# Usage:
#   scripts/test.sh             # run all tests
#   scripts/test.sh core        # run only vmctl-core tests
#   scripts/test.sh tui         # run only vmctl-tui tests
#   scripts/test.sh desktop     # run only vmctl-desktop tests

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TARGET="${1:-all}"

run() {
  label="$1"; shift
  echo "==> $label"
  "$@"
  echo ""
}

case "$TARGET" in
  -h|--help)
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 0 ;;
  core)
    run "vmctl-core tests" sh -c "cd '$ROOT' && cargo test -p vmctl-core" ;;
  tui)
    run "vmctl-tui tests" sh -c "cd '$ROOT' && cargo test -p vmctl-tui" ;;
  desktop)
    run "vmctl-desktop tests" sh -c "cd '$ROOT/desktop' && cargo test" ;;
  all)
    run "vmctl-core tests"   sh -c "cd '$ROOT' && cargo test -p vmctl-core"
    run "vmctl-tui tests"    sh -c "cd '$ROOT' && cargo test -p vmctl-tui"
    run "vmctl-desktop tests" sh -c "cd '$ROOT/desktop' && cargo test"
    echo "All test suites passed." ;;
  *)
    echo "test.sh: unknown target '$TARGET' (use: core|tui|desktop|all)" >&2
    exit 2 ;;
esac
