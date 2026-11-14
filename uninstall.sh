#!/bin/sh
# Remove what install.sh installed for the current user (~/.local).
# Usage: ./uninstall.sh          remove Voyager, keep its settings
#        ./uninstall.sh --purge  also delete its settings (~/.config/voyager)
# Use the same PREFIX as for installing, if you set one.
set -e
PREFIX="${PREFIX:-$HOME/.local}"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"

removed=0
for f in "$PREFIX/bin/voyager" "$PREFIX/share/applications/voyager.desktop" "$PREFIX/share/icons/hicolor/scalable/apps/voyager.svg" "$PREFIX/share/metainfo/io.github.HydrAPI.Voyager.metainfo.xml"; do
    if [ -e "$f" ]; then
        rm -f "$f"
        echo "Removed $f"
        removed=1
    fi
done
for mo in "$PREFIX"/share/locale/*/LC_MESSAGES/voyager.mo; do
    [ -e "$mo" ] && rm -f "$mo"
done
[ "$removed" = 1 ] || echo "Voyager isn't installed in $PREFIX."
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
[ -f "$PREFIX/share/icons/hicolor/icon-theme.cache" ] && gtk-update-icon-cache -q "$PREFIX/share/icons/hicolor" 2>/dev/null || true

# If --default made Voyager the folder handler, give folders back to the
# application it replaced, or else drop just that association so the
# desktop's own default applies again.
MIMEAPPS="$CONFIG/mimeapps.list"
PREVIOUS_FILE="$CONFIG/voyager/previous-folder-app"
if [ -f "$MIMEAPPS" ] && grep -q '^inode/directory=.*voyager\.desktop' "$MIMEAPPS"; then
    PREVIOUS="$(cat "$PREVIOUS_FILE" 2>/dev/null || true)"
    if [ -n "$PREVIOUS" ]; then
        xdg-mime default "$PREVIOUS" inode/directory
        echo "Folders open with $PREVIOUS again."
    else
        sed -i -e '/^inode\/directory=/ s/voyager\.desktop;\{0,1\}//' -e '/^inode\/directory=$/d' "$MIMEAPPS"
        echo "Voyager is no longer the default application for folders."
    fi
fi
rm -f "$PREVIOUS_FILE"

# Only removes the folder if nothing else is in it.
rmdir "$CONFIG/voyager" 2>/dev/null || true

if [ "$1" = "--purge" ]; then
    if [ -d "$CONFIG/voyager" ]; then
        rm -rf "$CONFIG/voyager"
        echo "Deleted the settings in $CONFIG/voyager"
    fi
else
    [ -d "$CONFIG/voyager" ] && echo "Kept the settings in $CONFIG/voyager (./uninstall.sh --purge deletes them)."
fi
exit 0
