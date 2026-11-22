#!/bin/sh
# Installs My Tools for the current user (no sudo needed).
#
#   ./install.sh               install or update
#   ./install.sh --uninstall   remove it again
set -eu

cd "$(dirname "$0")"

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
SIZES="16 24 32 48 64 128 256 512 1024"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/my-tools"
    rm -f "$DATA_DIR/applications/my-tools.desktop"
    rm -f "$DATA_DIR/icons/hicolor/scalable/apps/my-tools.svg"
    for s in $SIZES; do
        rm -f "$DATA_DIR/icons/hicolor/${s}x${s}/apps/my-tools.png"
    done
    echo "My Tools removed."
    exit 0
fi

install -Dm755 my-tools "$BIN_DIR/my-tools"
install -Dm644 my-tools.desktop "$DATA_DIR/applications/my-tools.desktop"
install -Dm644 icons/icon.svg "$DATA_DIR/icons/hicolor/scalable/apps/my-tools.svg"
for s in $SIZES; do
    install -Dm644 "icons/icon-$s.png" "$DATA_DIR/icons/hicolor/${s}x${s}/apps/my-tools.png"
done

# Ask the desktop to notice the new icon and menu entry (skipped if the tools aren't there).
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t "$DATA_DIR/icons/hicolor" || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$DATA_DIR/applications" || true
fi

echo "My Tools installed to $BIN_DIR/my-tools"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not on your PATH; add it so the menu entry can start the app." ;;
esac
