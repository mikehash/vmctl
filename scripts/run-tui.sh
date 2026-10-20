#!/bin/sh
# run-tui.sh — launch the vmctl terminal app.
#
# vm(8) needs root to manage guests, so this re-execs itself under sudo/doas
# when not already root. Read-only browsing works without root, but actions
# (start/stop/create/…) will fail — pass --no-root to stay unprivileged.
#
# Usage:
#   scripts/run-tui.sh              # run as root (via doas/sudo if needed)
#   scripts/run-tui.sh --no-root    # run unprivileged (read-only)
#   scripts/run-tui.sh --list       # non-interactive: print the VM list and exit
#   scripts/run-tui.sh -- <args>    # pass any extra args straight to vmctl-tui

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/release/vmctl-tui"

# Build first if the binary is missing.
if [ ! -x "$BIN" ]; then
  echo "==> vmctl-tui not built yet; building…" >&2
  "$ROOT/scripts/build.sh" tui
fi

# Re-exec as root unless we're already root or --no-root was given.
NEED_ROOT=1
for arg in "$@"; do
  case "$arg" in
    --no-root) NEED_ROOT=0 ;;
  esac
done

if [ "$NEED_ROOT" = "1" ] && [ "$(id -u)" -ne 0 ]; then
  if command -v doas >/dev/null 2>&1; then
    exec doas "$BIN" "$@"
  elif command -v sudo >/dev/null 2>&1; then
    exec sudo "$BIN" "$@"
  else
    echo "run-tui.sh: need root for vm(8) but neither doas nor sudo found." >&2
    echo "            Re-run as root, or use --no-root for read-only." >&2
    exit 1
  fi
fi

exec "$BIN" "$@"
