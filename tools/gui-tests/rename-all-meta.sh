#!/bin/bash
# rename-all-meta.sh x11|wayland: Rename All keeps the items' notes (and so
# their emblems and custom icons, kept the same way, in GVfs's metadata).
# Two files with a note each, renamed to upper case: the notes have to
# stay (renamed past GIO, GVfs lost them).
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/ren; mkdir -p "$T"; touch "$T/a.txt" "$T/b.txt"
gio set "$T/a.txt" metadata::annotation "note of a"
gio set "$T/b.txt" metadata::annotation "note of b"
gt_display
gt_arkha "$T"
gt_click 900 650; gt_key ctrl+a; gt_key F2; sleep 1
gt_ok "Upper case chosen" gt_a11y press "Upper case"
gt_ok "Applied" gt_a11y press Apply; sleep 1
gt_shot renamed
note() { gio info -a metadata::annotation "$1" 2>/dev/null | sed -n 's/^ *metadata::annotation: //p'; }
gt_check "renamed" "$(ls "$T" | tr '\n' ' ')" "A.TXT B.TXT "
gt_check "the notes are kept" "$(note "$T/A.TXT") | $(note "$T/B.TXT")" "note of a | note of b"
gt_end
