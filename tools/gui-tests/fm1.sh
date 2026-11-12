#!/bin/bash
# fm1.sh x11|wayland: "Show in folder" for other programs, through the
# org.freedesktop.FileManager1 D-Bus service Arkha offers: ShowItems opens
# the folder with the item selected, ShowFolders opens folders, and
# ShowItemProperties opens Properties.
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/fm1; mkdir -p "$T/A/sub" "$T/B"; echo a > "$T/A/alpha.txt"; echo b > "$T/A/beta.txt"
call() { gdbus call --session --dest org.freedesktop.FileManager1 --object-path /org/freedesktop/FileManager1 \
  --method "org.freedesktop.FileManager1.$1" "$2" "" >/dev/null 2>&1; sleep 1.5; gt_quiet; }
gt_display
gt_arkha "$T/B"
gt_check "the service is Arkha's" "$(gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
  --method org.freedesktop.DBus.GetConnectionUnixProcessID org.freedesktop.FileManager1 2>/dev/null | sed -n 's/.*uint32 \([0-9]*\).*/\1/p')" "$ARKHA_PID"
call ShowItems "['file://$T/A/beta.txt']"; gt_shot items
gt_check "ShowItems opens the item's folder" "$(gt_a11y windows | sort | tr '\n' ' ')" "A B "
call ShowFolders "['file://$T/A/sub', 'file://$T/B']"; gt_shot folders
gt_check "ShowFolders opens the folders (B shown already)" "$(gt_a11y windows | sort | tr '\n' ' ')" "A B sub "
call ShowItemProperties "['file://$T/A/alpha.txt']"; gt_shot properties
gt_check "ShowItemProperties opens Properties" "$(gt_a11y dialogs | grep -c alpha.txt)" "1"
gt_end
