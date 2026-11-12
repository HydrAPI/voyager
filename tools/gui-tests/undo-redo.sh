#!/bin/bash
# undo-redo.sh x11|wayland: Undo and Redo of a move to the trash, of a new
# folder named at once (one step), of a rename, and of renaming that new
# folder later (its own step: undone, it gets its name back), with real
# keys (Ctrl+Z, Shift+Ctrl+Z), checked on the files.
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/ur; TRASH=$HOME/.local/share/Trash/files
mkdir -p "$T"; echo a > "$T/a.txt"; echo b > "$T/b.txt"
state() { echo "$(cd "$T" && ls | tr '\n' ' ')| trash: $(ls "$TRASH" 2>/dev/null | tr '\n' ' ')"; }
gt_display
gt_arkha "$T"
gt_click 900 650
gt_key Home
gt_key Delete; sleep 0.5; gt_key Return
gt_check "a.txt trashed" "$(state)" "b.txt | trash: a.txt "
gt_key ctrl+z; sleep 1; gt_quiet
gt_check "undo: back from the trash" "$(state)" "a.txt b.txt | trash: "
gt_key shift+ctrl+z; sleep 1; gt_quiet
gt_check "redo: trashed again" "$(state)" "b.txt | trash: a.txt "
gt_key ctrl+z; sleep 1; gt_quiet
gt_key shift+ctrl+n; gt_type Photos; gt_key Return
gt_check "a new folder named at once" "$(state)" "a.txt b.txt Photos | trash: "
gt_key ctrl+z; sleep 1; gt_quiet
gt_check "undo: it goes to the trash, in one step" "$(state)" "a.txt b.txt | trash: Photos "
gt_key shift+ctrl+z; sleep 1; gt_quiet
gt_check "redo: made again" "$(state)" "a.txt b.txt Photos | trash: Photos "
rm -rf "$TRASH" "$HOME/.local/share/Trash/info"
gt_type a.t; gt_key Escape; gt_key F2; gt_type first; gt_key Return
gt_check "renamed" "$(state)" "b.txt first.txt Photos | trash: "
gt_key ctrl+z; sleep 0.5; gt_quiet
gt_check "undo: the old name" "$(state)" "a.txt b.txt Photos | trash: "
gt_key shift+ctrl+z; sleep 0.5; gt_quiet
gt_check "redo: the new name" "$(state)" "b.txt first.txt Photos | trash: "
gt_type Ph; gt_key Escape; gt_key F2; gt_type Pictures; gt_key Return
gt_check "the new folder renamed later" "$(state)" "b.txt first.txt Pictures | trash: "
gt_key ctrl+z; sleep 0.5; gt_quiet
gt_check "undo: its name back, the folder kept" "$(state)" "b.txt first.txt Photos | trash: "
gt_shot end
gt_end
