#!/bin/bash
# rename-linebreak.sh x11|wayland: F2 and Enter, nothing typed, leave a
# name as it is, also one with a line break in it (the field leaves line
# breaks out, and the name it showed was taken for a new one).
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/ren; mkdir -p "$T"; touch "$T/two
lines.txt"
gt_display
gt_arkha "$T"
gt_click 900 650; gt_key Home; gt_key F2; sleep 0.5; gt_key Return; sleep 1
gt_check "the name is left as it is" "$(ls -b "$T")" 'two\nlines.txt'
gt_end
