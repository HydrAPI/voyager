#!/bin/bash
# backgrounds.sh x11|wayland: Backgrounds and Emblems opens (a screenshot of
# each tab) and closes with Escape.
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/bg; mkdir -p "$T/docs"; for n in alpha beta; do echo $n > "$T/$n.txt"; done
gt_display
gt_arkha "$T"
gt_act backgrounds
gt_check "it opens" "$(gt_a11y dialogs)" "Backgrounds and Emblems"
gt_shot patterns
gt_key ctrl+Page_Down; gt_shot colours
gt_key ctrl+Page_Down; gt_shot emblems
gt_key Escape
gt_check "Escape closes it" "$(gt_a11y dialogs)" ""
gt_end
