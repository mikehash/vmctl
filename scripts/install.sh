#!/bin/sh
# install.sh — build both apps and install them onto your PATH.
#
# Installs the two binaries plus thin launcher wrappers into a bin directory
# (default: /usr/local/bin, or $PREFIX/bin). After this you can just run
# `vmctl-tui` and `vmctl-desktop` from anywhere.
#
# Usage:
#   scripts/install.sh                  # install to /usr/local/bin (needs root)
#   scripts/install.sh --prefix ~/.local # install to ~/.local/bin (no root)
#   scripts/install.sh --uninstall      # remove the installed files

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX="/usr/local"
UNINSTALL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --prefix=*) PREFIX="${1#*=}"; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done

BIN_DIR="$PREFIX/bin"
TUI_BIN="$ROOT/target/release/vmctl-tui"
DESK_BIN="$ROOT/desktop/target/release/vmctl-desktop"

remove() {
  echo "==> Removing vmctl from $BIN_DIR"
  rm -f "$BIN_DIR/vmctl-tui" "$BIN_DIR/vmctl-desktop" "$BIN_DIR/vmctl"
  echo "Removed."
}

if [ "$UNINSTALL" = "1" ]; then
  remove
  exit 0
fi

# Build release binaries first.
"$ROOT/scripts/build.sh"

echo "==> Installing to $BIN_DIR"
mkdir -p "$BIN_DIR"

install -m 0755 "$TUI_BIN"  "$BIN_DIR/vmctl-tui"
install -m 0755 "$DESK_BIN" "$BIN_DIR/vmctl-desktop"

# A tiny `vmctl` dispatcher so one command reaches either app.
cat > "$BIN_DIR/vmctl" <<EOF
#!/bin/sh
# vmctl — dispatch to the TUI or Desktop front-end.
#   vmctl tui [args]      → terminal app
#   vmctl desktop [args]  → desktop app
#   vmctl build|test      → build / test both apps
case "\${1:-}" in
  tui|terminal) shift; exec vmctl-tui "\$@" ;;
  desktop|gui)  shift; exec vmctl-desktop "\$@" ;;
  build)        shift; exec "$ROOT/scripts/build.sh" "\$@" ;;
  test)         shift; exec "$ROOT/scripts/test.sh" "\$@" ;;
  *)
    echo "vmctl — friendly vm-bhyve front-ends"
    echo ""
    echo "  vmctl tui [args]       launch the terminal app"
    echo "  vmctl desktop [args]   launch the desktop app"
    echo "  vmctl build [target]   build both apps"
    echo "  vmctl test [target]    run the test suite"
    exit 0 ;;
esac
EOF
chmod 0755 "$BIN_DIR/vmctl"

echo ""
echo "Installed:"
echo "  $BIN_DIR/vmctl-tui"
echo "  $BIN_DIR/vmctl-desktop"
echo "  $BIN_DIR/vmctl"
echo ""
echo "Try:  vmctl tui        (terminal)"
echo "      vmctl desktop    (desktop, needs a display)"
