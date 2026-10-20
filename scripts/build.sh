#!/bin/sh
# build.sh — build both vmctl apps (TUI + Desktop) in release mode.
#
# The two front-ends live in separate Cargo workspaces (Slint's dependency
# tree conflicts with ratatui's pinned unicode-width), so we build each in
# turn. Both share vmctl-core by path, so behavior stays identical.
#
# Usage:
#   scripts/build.sh            # build both apps
#   scripts/build.sh tui        # build only the TUI
#   scripts/build.sh desktop    # build only the Desktop
#   scripts/build.sh --debug    # build in debug (faster compile, slower run)

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PROFILE="release"
CARGO_FLAGS="--release"

# Parse args: optional --debug flag, optional target filter.
TARGET="all"
for arg in "$@"; do
  case "$arg" in
    --debug) PROFILE="debug"; CARGO_FLAGS="" ;;
    tui|desktop|core|all) TARGET="$arg" ;;
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "build.sh: unknown argument '$arg'" >&2; exit 2 ;;
  esac
done

build_root() {
  echo "==> Building shared core + TUI ($PROFILE)"
  ( cd "$ROOT" && cargo build $CARGO_FLAGS )
}

build_desktop() {
  echo "==> Building Desktop / Slint ($PROFILE)"
  ( cd "$ROOT/desktop" && cargo build $CARGO_FLAGS )
}

case "$TARGET" in
  core)     echo "==> Building shared core ($PROFILE)"; ( cd "$ROOT" && cargo build $CARGO_FLAGS -p vmctl-core ) ;;
  tui)      build_root ;;
  desktop)  build_desktop ;;
  all)      build_root; build_desktop ;;
esac

echo ""
echo "Build complete ($PROFILE)."
if [ "$PROFILE" = "release" ]; then
  echo "  TUI:     $ROOT/target/release/vmctl-tui"
  echo "  Desktop: $ROOT/desktop/target/release/vmctl-desktop"
else
  echo "  TUI:     $ROOT/target/debug/vmctl-tui"
  echo "  Desktop: $ROOT/desktop/target/debug/vmctl-desktop"
fi
