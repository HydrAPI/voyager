#!/bin/bash
# columns.sh x11|wayland: the list view's columns and sort orders. The
# default columns; one added from a column header's menu (kept in the
# settings); sorting by permissions; the Trash's own columns; icon
# captions (a screenshot).
GT_A11Y=1 GT_CONF='per_folder=false'
. "$(dirname "$0")/lib.sh"
gt_session "$@"

T=$HOME/cols; mkdir -p "$T"
for n in a b c; do echo "$n" > "$T/$n.txt"; done
chmod 644 "$T/a.txt"; chmod 600 "$T/b.txt"; chmod 755 "$T/c.txt"
echo x > "$HOME/gone.txt"
gt_display
gt_arkha "$T"
gt_act view-mode "<'list'>"
headers() { gt_a11y headers | tr '\n' ' '; }
names() { gt_a11y cells | grep '\.txt$' | tr '\n' ' '; }
gt_check "the default columns" "$(headers)" "Name Size Type Modified "
gt_shot default
# The header menu: Size, Type, Modified, Accessed, Created, Owner ...
y=120
gt_click 700 $y 3; gt_shot header-menu
gt_key Down Down Down Down Down Down Return
gt_check "Owner added from the header menu" "$(headers)" "Name Size Type Modified Owner "
gt_check "and kept" "$(grep '^list_columns=' "$XDG_CONFIG_HOME/arkha/arkha.conf")" "list_columns=size;type;mtime;owner;"
gt_act sort-by "<'permissions'>"; gt_shot sorted
gt_check "sorted by permissions (600, 644, 755)" "$(names)" "b.txt a.txt c.txt "
gt_act sort-by "<'name'>"
gt_check "by name again" "$(names)" "a.txt b.txt c.txt "
gio trash "$HOME/gone.txt"
gt_act go-trash; gt_shot trash
gt_check "the Trash's columns" "$(headers)" "Name Size Type Modified Owner Trashed On Original Location "
gt_act back
gt_check "and only there" "$(headers)" "Name Size Type Modified Owner "
gt_act view-mode "<'icon'>"; gt_act zoom-in; gt_act zoom-in; gt_act zoom-in; gt_shot captions
gt_end
