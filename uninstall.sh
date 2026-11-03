#!/bin/sh
# Remove what install.sh installed for the current user (~/.local).
# Usage: ./uninstall.sh          remove Arkha, keep its settings
#        ./uninstall.sh --purge  also delete its settings (~/.config/arkha)
# Use the same PREFIX as for installing, if you set one.
set -e
PREFIX="${PREFIX:-$HOME/.local}"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"

removed=0
for f in "$PREFIX/bin/arkha" "$PREFIX/share/applications/arkha.desktop"; do
    if [ -e "$f" ]; then
        rm -f "$f"
        echo "Removed $f"
        removed=1
    fi
done
[ "$removed" = 1 ] || echo "Arkha isn't installed in $PREFIX."
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true

# Folders and automounting go back to the desktop.
sh "$(dirname "$0")/arkha-restore-desktop"

# Only removes the folder if nothing else is in it.
rmdir "$CONFIG/arkha" 2>/dev/null || true

if [ "$1" = "--purge" ]; then
    if [ -d "$CONFIG/arkha" ]; then
        rm -rf "$CONFIG/arkha"
        echo "Deleted the settings in $CONFIG/arkha"
    fi
else
    [ -d "$CONFIG/arkha" ] && echo "Kept the settings in $CONFIG/arkha (./uninstall.sh --purge deletes them)."
fi
exit 0
