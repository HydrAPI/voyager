#!/bin/sh
# Build and install Arkha for the current user (~/.local).
# Usage: ./install.sh            install
#        ./install.sh --default  install and make it the default folder handler
set -e
cd "$(dirname "$0")"
cargo build --release
PREFIX="${PREFIX:-$HOME/.local}"
install -Dm755 target/release/arkha "$PREFIX/bin/arkha"
install -Dm644 arkha.desktop "$PREFIX/share/applications/arkha.desktop"
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
if [ "$1" = "--default" ]; then
    # Remember the current folder handler, so uninstall.sh can restore it.
    PREVIOUS="$(xdg-mime query default inode/directory 2>/dev/null || true)"
    if [ -n "$PREVIOUS" ] && [ "$PREVIOUS" != "arkha.desktop" ]; then
        CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/arkha"
        mkdir -p "$CONFIG"
        echo "$PREVIOUS" > "$CONFIG/previous-folder-app"
    fi
    xdg-mime default arkha.desktop inode/directory
    echo "Arkha is now the default application for folders."
fi
echo "Installed to $PREFIX/bin/arkha"
