#!/bin/bash
# tabs.sh x11|wayland: where new tabs go (by default after the current one,
# in the order opened). In top/: alpha, bravo and charlie opened in new
# tabs, then alpha's tab chosen and its xray and yankee opened in new tabs,
# then a new tab (Ctrl+T, alpha again: at the end, as a browser puts it).
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

mkdir -p "$HOME/top"/{alpha/xray,alpha/yankee,bravo,charlie}
gt_display
gt_voyager "$HOME/top"
pick() { gt_type "$1"; sleep 0.4; gt_key Escape; }
for n in alpha bravo charlie; do pick $n; gt_act open-new-tab; done
gt_shot first
gt_check "folders open in tabs after the current one, in order" "$(gt_a11y tabs | tr '\n' ' ')" "top alpha bravo charlie "
gt_key ctrl+Page_Down
for n in xray yankee; do pick $n; gt_act open-new-tab; done
gt_act new-tab
gt_shot second
gt_check "then after alpha's tab, in order; Ctrl+T at the end" "$(gt_a11y tabs | tr '\n' ' ')" "top alpha xray yankee bravo charlie alpha "
gt_end
