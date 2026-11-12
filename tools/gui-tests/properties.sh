#!/bin/bash
# properties.sh x11|wayland: Properties of a file: an emblem ticked on its
# Emblems tab and a note typed on its Notes tab are kept in GVfs metadata
# under Caja's keys (read back with gio, as Caja would); one set by Caja's
# way (gio) before is shown ticked and kept.
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/props; mkdir -p "$T"; for n in alpha beta gamma; do echo $n > "$T/$n.txt"; done
meta() { gio info -a "metadata::$2" "$T/$1" | sed -n "s/^ *metadata::$2: //p"; }
gt_display
gio set -t stringv "$T/gamma.txt" metadata::emblems important
gt_arkha "$T"
gt_click 900 650
gt_type gam; gt_key Escape
gt_act properties; sleep 1; gt_shot basic
gt_check "Properties opens" "$(gt_a11y dialogs | grep -c gamma)" "1"
gt_ok "the Emblems tab" gt_a11y tab Emblems; sleep 0.5; gt_quiet; gt_shot emblems
gt_ok "tick Readonly" gt_a11y press Readonly; sleep 0.5
gt_ok "the Notes tab" gt_a11y tab Notes; sleep 0.5; gt_quiet
gt_ok "into the note" gt_a11y focus text; gt_type "Check this one."; gt_shot notes
gt_key Escape; sleep 1.5
gt_check "the emblems kept (Caja's key)" "$(meta gamma.txt emblems)" "[important, readonly]"
gt_check "the note kept (Caja's key)" "$(meta gamma.txt annotation)" "Check this one."
gt_shot after
gt_end
