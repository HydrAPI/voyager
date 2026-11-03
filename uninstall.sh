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

# If install.sh --default made Arkha the folder handler, give folders back
# to the application it replaced, or else drop just that association so the
# desktop's own default applies again.
MIMEAPPS="$CONFIG/mimeapps.list"
PREVIOUS_FILE="$CONFIG/arkha/previous-folder-app"
if [ -f "$MIMEAPPS" ] && grep -q '^inode/directory=.*arkha\.desktop' "$MIMEAPPS"; then
    PREVIOUS="$(cat "$PREVIOUS_FILE" 2>/dev/null || true)"
    if [ -n "$PREVIOUS" ]; then
        xdg-mime default "$PREVIOUS" inode/directory
        echo "Folders open with $PREVIOUS again."
    else
        sed -i -e '/^inode\/directory=/ s/arkha\.desktop;\{0,1\}//' -e '/^inode\/directory=$/d' "$MIMEAPPS"
        echo "Arkha is no longer the default application for folders."
    fi
fi
rm -f "$PREVIOUS_FILE"

# Arkha mounts inserted drives itself and switches the desktop's own
# automounting off; switch it back on where Arkha did that.
AUTOMOUNT_FILE="$CONFIG/arkha/desktop-automount"
if [ -f "$AUTOMOUNT_FILE" ]; then
    KWRITE="$(command -v kwriteconfig6 || command -v kwriteconfig5 || true)"
    while read -r kind a b c d; do
        case "$kind" in
            gsettings) gsettings set "$a" "$b" "$c" 2>/dev/null || true ;;
            xfconf) xfconf-query -c "$a" -p "$b" -s "$c" 2>/dev/null || true ;;
            ini)
                # PCManFM(-Qt): section, key, old value (- = wasn't set), file.
                if [ -f "$d" ]; then
                    awk -v sec="[$a]" -v key="$b" -v val="$c" '
                        /^[[:space:]]*\[/ { inside = ($0 == sec) }
                        inside && index($0, key "=") == 1 { if (val != "-") print key "=" val; next }
                        { print }' "$d" > "$d.arkha-tmp" && mv "$d.arkha-tmp" "$d"
                fi
                ;;
            kde)
                if [ -n "$KWRITE" ]; then
                    if [ "$d" = "-" ]; then
                        "$KWRITE" --file "$a" --group "$b" --key "$c" --delete || true
                    else
                        "$KWRITE" --file "$a" --group "$b" --key "$c" --type bool "$d" || true
                    fi
                fi
                ;;
        esac
    done < "$AUTOMOUNT_FILE"
    rm -f "$AUTOMOUNT_FILE"
    echo "The desktop mounts inserted drives itself again."
fi
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
