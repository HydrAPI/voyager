#!/bin/sh
# Build and install Voyager for the current user (~/.local).
# Usage: ./install.sh            install
#        ./install.sh --default  install and make it the default folder handler
set -e
cd "$(dirname "$0")"
cargo build --release
PREFIX="${PREFIX:-$HOME/.local}"
install -Dm755 target/release/voyager "$PREFIX/bin/voyager"
install -Dm644 voyager.desktop "$PREFIX/share/applications/voyager.desktop"
# The desktop's launchers (Win+E, file dialogs) may not have ~/.local/bin in
# their PATH: the full path of the program goes in the launcher.
sed -i "s|^Exec=voyager|Exec=$PREFIX/bin/voyager|" "$PREFIX/share/applications/voyager.desktop"
install -Dm644 data/voyager.svg "$PREFIX/share/icons/hicolor/scalable/apps/voyager.svg"
install -Dm644 data/io.github.HydrAPI.Voyager.metainfo.xml "$PREFIX/share/metainfo/io.github.HydrAPI.Voyager.metainfo.xml"
# The translations the build made from po/*.po (none without gettext).
for mo in target/release/locale/*/LC_MESSAGES/voyager.mo; do
    [ -e "$mo" ] && install -Dm644 "$mo" "$PREFIX/share/locale/${mo#target/release/locale/}"
done
# Only where the theme has a cache, which then has to know the new icon.
[ -f "$PREFIX/share/icons/hicolor/icon-theme.cache" ] && gtk-update-icon-cache -q "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
if [ "$1" = "--default" ]; then
    # Remember the current folder handler, so uninstall.sh can restore it.
    PREVIOUS="$(xdg-mime query default inode/directory 2>/dev/null || true)"
    if [ -n "$PREVIOUS" ] && [ "$PREVIOUS" != "voyager.desktop" ]; then
        CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/voyager"
        mkdir -p "$CONFIG"
        echo "$PREVIOUS" > "$CONFIG/previous-folder-app"
    fi
    xdg-mime default voyager.desktop inode/directory
    echo "Voyager is now the default application for folders."
fi
echo "Installed to $PREFIX/bin/voyager"
