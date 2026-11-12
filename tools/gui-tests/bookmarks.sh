#!/bin/bash
# bookmarks.sh x11|wayland: Bookmarks → Edit Bookmarks shows locations as
# people know them (a folder's path, not file:///...; a network place's
# address without %20), while the bookmarks file keeps the URLs: a
# bookmark removed in the editor leaves the others as they were written.
GT_A11Y=1
. "$(dirname "$0")/lib.sh"
gt_session "$@"

mkdir -p "$HOME/Backups" "$HOME/My Files" "$XDG_CONFIG_HOME/gtk-3.0"
B=$XDG_CONFIG_HOME/gtk-3.0/bookmarks
printf 'file://%s/Backups\nfile://%s/My%%20Files Mine\nsmb://server/My%%20Share/\n' "$HOME" "$HOME" > "$B"
gt_display
gt_arkha "$HOME"
gdbus call --session --dest io.github.arkha.FileManager --object-path $APP_PATH/window/1 \
  --method org.gtk.Actions.Activate edit-bookmarks [] {} >/dev/null & sleep 2
gt_shot editor
cells() { python3 - <<'PY'
import gi; gi.require_version('Atspi', '2.0'); from gi.repository import Atspi
Atspi.init(); d = Atspi.get_desktop(0)
app = [d.get_child_at_index(i) for i in range(d.get_child_count()) if d.get_child_at_index(i).get_name() == 'arkha'][0]
dlg = [app.get_child_at_index(i) for i in range(app.get_child_count()) if app.get_child_at_index(i).get_role_name() == 'dialog'][0]
stack = [dlg]
while stack:
    x = stack.pop()
    if x.get_role_name() == 'table cell' and x.get_name():
        print(x.get_name())
    stack.extend(c for c in (x.get_child_at_index(k) for k in range(x.get_child_count() - 1, -1, -1)) if c)
PY
}
gt_check "the editor shows paths and plain addresses" "$(cells | tr '\n' '|')" "Backups|$HOME/Backups|Mine|$HOME/My Files|My Share|smb://server/My Share/|"
# The first one removed (selected with the keyboard): the rest written as before.
gt_key Tab; gt_key Down; gt_key Up
gt_ok "Remove" gt_a11y press Remove; sleep 0.5
gt_check "the bookmarks file keeps the URLs" "$(cat "$B" | tr '\n' '|')" "file://$HOME/My%20Files Mine|smb://server/My%20Share/|"
gt_shot removed
gt_key Escape
gt_end
