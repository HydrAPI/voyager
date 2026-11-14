#!/bin/bash
# speed.sh OLD NEW [DIRS...]: opening folders with two Voyager builds (say
# the last package's and this one), alternating, 3 runs each: when the main
# thread stays idle, its CPU and all threads', in the icon view, the list
# view, and the list view with every column. Default folders: /usr/lib and
# 20,000 files. A change that should cost nothing must show the same.
GT_BENCH=1 GT_WM=none
D=$(cd "$(dirname "$0")" && pwd)
[ -n "$GT_INNER" ] || [ $# -ge 2 ] || { echo "usage: $0 OLD-BINARY NEW-BINARY [DIRS...]" >&2; exit 2; }
"$D/../gui-tests/fixtures.sh" bench >/dev/null || exit 2
. "$D/../gui-tests/lib.sh"
GT_NAME=speed
if [ -z "$GT_INNER" ]; then
  OLD=$(realpath "$1") NEW=$(realpath "$2"); export OLD NEW
  shift 2; export DIRS="${*:-/usr/lib $GT_FIX/big20k}"
fi
gt_session x11

gt_display 1280 800
for dir in $DIRS; do
  for view in icon list all; do
    for bin in old new old new old new; do
      case $view in
        icon) printf '[General]\nper_folder=false\n' ;;
        list) printf '[General]\nper_folder=false\ndefault_view=list\n' ;;
        all) printf '[General]\nper_folder=false\ndefault_view=list\nlist_columns=size;type;mtime;atime;btime;owner;group;permissions;octal;mime;extension;location;disk;\n' ;;
      esac > "$XDG_CONFIG_HOME/voyager/voyager.conf"
      b=$OLD; [ $bin = new ] && b=$NEW
      t0=$(date +%s%N); "$b" "$dir" >> "$OUT/voyager-$bin.log" 2>&1 & P=$!
      last=0 quiet=0 busy=0
      while true; do
        sleep 0.1
        t=$(awk '{print $14+$15}' /proc/$P/task/$P/stat 2>/dev/null) || break
        all=$(awk '{print $14+$15}' /proc/$P/stat 2>/dev/null)
        now=$(( ($(date +%s%N) - t0) / 1000000 ))
        if [ "$t" = "$last" ]; then quiet=$((quiet+1)); else quiet=0; busy=$now; fi; last=$t
        { [ $quiet -ge 6 ] && [ $now -gt 1500 ]; } || [ $now -gt 30000 ] && break
      done
      gt_note "$(basename "$dir") $view $bin: main thread busy until $busy ms, cpu $((t*10)) ms, all threads $((all*10)) ms"
      kill $P 2>/dev/null; wait $P 2>/dev/null; sleep 1
    done
  done
done
gt_end
