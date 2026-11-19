#!/bin/sh
# build.sh: builds Voyager's .deb, on Debian 13 or newer (or a relative such
# as Ubuntu 26.04): run in a copy of the sources made by build-on.sh (the
# crates in vendor/, built without internet; the version in VERSION), or
# in a checkout with internet for the crates. Needs cargo and rustc 1.85+,
# libgtk-3-dev, gettext, pkg-config, build-essential and dpkg-dev. The
# package goes to target/deb/.
set -e
cd "$(dirname "$0")/../.."
top=$(pwd)
version=$(sh packaging/version.sh deb)
arch=$(dpkg --print-architecture)
offline=
[ -d vendor ] && offline=--offline
cargo build --release --locked $offline

out=$top/target/deb
stage=$out/voyager_${version}-1_${arch}
rm -rf "$out"
mkdir -p "$stage/DEBIAN"
install -Dm755 target/release/voyager "$stage/usr/bin/voyager"
install -Dm644 voyager.desktop "$stage/usr/share/applications/voyager.desktop"
install -Dm644 data/voyager.svg "$stage/usr/share/icons/hicolor/scalable/apps/voyager.svg"
install -Dm644 data/io.github.HydrAPI.Voyager.metainfo.xml "$stage/usr/share/metainfo/io.github.HydrAPI.Voyager.metainfo.xml"
# The translations build.rs made from po/*.po.
for mo in target/release/locale/*/LC_MESSAGES/voyager.mo; do
    [ -e "$mo" ] && install -Dm644 "$mo" "$stage/usr/share/locale/${mo#target/release/locale/}"
done
install -Dm644 README.md "$stage/usr/share/doc/voyager/README.md"
# Debian keeps the GPL's text in base-files' /usr/share/common-licenses.
cat > "$stage/usr/share/doc/voyager/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Voyager
Source: https://github.com/HydrAPI/voyager

Files: *
Copyright: 2026 HydrAPI, dekart811 (Arkha)
License: GPL-3.0+
Comment: Some texts and the search's groups of file types come from Caja
 (GPL-2.0-or-later), by the MATE developers.

License: GPL-3.0+
 This program is free software: you can redistribute it and/or modify it
 under the terms of the GNU General Public License as published by the Free
 Software Foundation, either version 3 of the License, or (at your option)
 any later version.
 .
 On Debian systems, the complete text of the GNU General Public License
 version 3 can be found in /usr/share/common-licenses/GPL-3.
EOF

# The libraries it's linked with, by their Debian package names and the
# versions it needs (dpkg-shlibdeps wants a debian/control to run in).
mkdir -p "$out/debian"
printf 'Source: voyager\n' > "$out/debian/control"
depends=$(cd "$out" && dpkg-shlibdeps -O -e "$stage/usr/bin/voyager" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
[ -n "$depends" ] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }

# The menu entry's and the icon's caches are updated by dpkg's triggers
# (desktop-file-utils, hicolor-icon-theme): no maintainer scripts needed.
cat > "$stage/DEBIAN/control" <<EOF
Package: voyager
Version: ${version}-1
Architecture: ${arch}
Maintainer: ${DEB_MAINTAINER:-HydrAPI <245955427+HydrAPI@users.noreply.github.com>}
Installed-Size: $(du -sk --exclude=DEBIAN "$stage" | cut -f1)
Depends: ${depends}
Recommends: gvfs, bubblewrap
Suggests: gvfs-backends, ffmpegthumbnailer | ffmpeg, mediainfo, engrampa | file-roller | xarchiver
Section: utils
Priority: optional
Description: fast graphical file manager
 Voyager is a file manager for GTK desktops: icon, list, compact and columns
 views, tabs and split view, an info panel, bulk renaming, checksums,
 checked copies, custom actions and thumbnailers, and undo for file
 operations. It uses the desktop's GTK theme, icons and bookmarks, and
 shares notes, emblems and custom icons with Caja.
EOF
dpkg-deb --root-owner-group -Zxz --build "$stage" "$out/" >/dev/null
ls "$out"/*.deb
