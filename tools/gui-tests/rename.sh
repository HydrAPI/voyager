#!/bin/bash
# rename.sh x11|wayland: renaming in the icon view: confirmed with Enter,
# cancelled with Escape, confirmed by a click elsewhere, and the folder
# reloaded while renaming (no warnings, the name typed so far kept or
# dropped, never a crash).
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/ren; mkdir -p "$T"; touch "$T/a.txt" "$T/b.txt" "$T/c.txt" "$T/d.txt"
gt_display
gt_arkha "$T"
gt_click 900 650
gt_key Home; gt_key F2; gt_type one; gt_key Return
gt_check "renamed with Enter" "$(ls "$T" | tr '\n' ' ')" "b.txt c.txt d.txt one.txt "
gt_key Home; gt_key F2; gt_type zzz; gt_key Escape
gt_check "Escape cancels" "$(ls "$T" | tr '\n' ' ')" "b.txt c.txt d.txt one.txt "
gt_key End; gt_key F2; gt_type two; gt_click 900 650
gt_check "a click elsewhere confirms" "$(ls "$T" | tr '\n' ' ')" "b.txt c.txt d.txt two.txt "
gt_key Home; gt_key F2; gt_type three; gt_act reload; sleep 1
case "$(ls "$T" | tr '\n' ' ')" in
  "b.txt c.txt d.txt two.txt "|"c.txt d.txt three.txt two.txt ") echo "PASS reloaded while renaming" >> "$OUT/result" ;;
  *) gt_check "reloaded while renaming" "$(ls "$T" | tr '\n' ' ')" "b.txt c.txt d.txt two.txt " ;;
esac
gt_ok "Arkha still running" kill -0 "$ARKHA_PID"
gt_shot end
gt_end
