#!/bin/sh
# run-desktop.sh — launch the vmctl desktop (Slint) app.
#
# Two things the desktop needs that the TUI doesn't:
#   1. A display. This runs the app inside your graphical session (X or
#      Wayland). It will not show a window from a headless SSH shell.
#   2. Root for vm(8) actions. Re-execs under doas/sudo like the TUI.
#
# If you launch from a plain SSH shell and get an X auth error
# ("Invalid MIT-MAGIC-COOKIE-1"), the DISPLAY's .Xauthority cookie is stale —
# launch from a terminal inside the desktop session instead, or refresh the
# cookie (see docs). This script checks the display and tells you clearly.
#
# Usage:
#   scripts/run-desktop.sh              # run as root in the current display
#   scripts/run-desktop.sh --no-root    # run unprivileged (read-only)
#   scripts/run-desktop.sh -- <args>    # pass extra args to vmctl-desktop

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/desktop/target/release/vmctl-desktop"

# Build first if the binary is missing.
if [ ! -x "$BIN" ]; then
  echo "==> vmctl-desktop not built yet; building…" >&2
  "$ROOT/scripts/build.sh" desktop
fi

# Display check: X needs DISPLAY, Wayland needs WAYLAND_DISPLAY.
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
  echo "run-desktop.sh: no display found (DISPLAY and WAYLAND_DISPLAY both unset)." >&2
  echo "                The desktop app needs a graphical session to show its window." >&2
  echo "                Launch this from a terminal on your desktop, or use run-tui.sh." >&2
  exit 1
fi

# Re-exec as root unless we're already root or --no-root was given.
NEED_ROOT=1
for arg in "$@"; do
  case "$arg" in
    --no-root) NEED_ROOT=0 ;;
  esac
done

if [ "$NEED_ROOT" = "1" ] && [ "$(id -u)" -ne 0 ]; then
  # Preserve the display environment across the privilege bump.
  if command -v doas >/dev/null 2>&1; then
    exec doas env DISPLAY="${DISPLAY:-}" WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-}" \
      XAUTHORITY="${XAUTHORITY:-$HOME/.Xauthority}" "$BIN" "$@"
  elif command -v sudo >/dev/null 2>&1; then
    exec sudo -E "$BIN" "$@"
  else
    echo "run-desktop.sh: need root for vm(8) but neither doas nor sudo found." >&2
    echo "                Re-run as root, or use --no-root for read-only." >&2
    exit 1
  fi
fi

exec "$BIN" "$@"
