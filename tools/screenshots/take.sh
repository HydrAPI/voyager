#!/bin/bash
# take.sh [light|dark ...]: takes the README's screenshots again, in both
# themes (or the ones named), and puts them in docs/screenshots/. Voyager
# runs headless in a made-up home folder (make-home.sh, in
# target/screenshots/home), as the GUI tests run it (tools/gui-tests),
# under Metacity for its Adwaita title bars; each window is cut out with
# its shadow, transparent around it. Light is Adwaita with Papirus, dark
# Adwaita-dark with Papirus-Dark, in Cantarell, all in English.
#
# Build first (cargo build --release). Needs what the GUI tests need, and
# metacity, the Papirus icon themes, the Cantarell font, the en_US.UTF-8
# locale, ImageMagick (magick) and python-gobject. Takes about 3 minutes.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
themes=${*:-light dark}
for t in $themes; do
    case $t in light|dark) ;; *) echo "usage: take.sh [light|dark ...]" >&2; exit 2 ;; esac
done
bash "$here/make-home.sh" "$root/target/screenshots/home"
mkdir -p "$root/docs/screenshots"
for t in $themes; do
    bash "$here/shots.sh" x11 "$t"
    out=$root/target/gui-tests/screenshots-$t-x11
    for n in thispc icons split rename checksums; do
        cp "$out/$n.png" "$root/docs/screenshots/$n-$t.png"
    done
done
ls -l "$root/docs/screenshots"
