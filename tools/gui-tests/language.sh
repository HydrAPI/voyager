#!/bin/bash
# language.sh x11|wayland: the language. With German chosen in Preferences
# (language=de in the settings) Arkha's texts are German, the status bar's
# with its plural and the side pane's; with the system's language (English
# in the tests) they're English again. Read through accessibility.
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

mkdir -p "$HOME/two"; touch "$HOME/two/a.txt" "$HOME/two/b.txt"
gt_display
labels() { gt_a11y window-labels | tr '\n' '|'; }
has() { case "$1" in *"|$2|"*) echo yes ;; *) echo "no: $1" ;; esac; }

printf '[General]\nlanguage=de\n' > "$XDG_CONFIG_HOME/arkha/arkha.conf"
gt_arkha "$HOME/two"
gt_shot german
l="|$(labels)"
case "$l" in *"|2 Objekte, freier Speicher: "*) s=yes ;; *) s="no: $l" ;; esac
gt_check "German: the status bar, with its plural" "$s" "yes"
gt_check "German: the side pane" "$(has "$l" "Persönlicher Ordner")" "yes"
gt_quit

printf '[General]\nlanguage=\n' > "$XDG_CONFIG_HOME/arkha/arkha.conf"
gt_arkha "$HOME/two"
gt_shot english
l="|$(labels)"
case "$l" in *"|2 items, Free space: "*) s=yes ;; *) s="no: $l" ;; esac
gt_check "the system's language (English): the status bar" "$s" "yes"
gt_check "the system's language: the side pane" "$(has "$l" "Home")" "yes"
gt_end
