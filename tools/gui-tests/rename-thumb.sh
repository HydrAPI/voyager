#!/bin/bash
# rename-thumb.sh x11|wayland: while an item is renamed in the icon view,
# its picture stays (the name's field goes below it). A video with a
# thumbnail and a name of four lines: F2, and the picture above the field
# has to be the same as before (the item was laid out with an empty name
# for that, its picture moving down behind the field).
GT_CONF='show_thumbnails=always'
. "$(dirname "$0")/lib.sh"
gt_session "$@"

mkdir -p "$HOME/videos"
ffmpeg -loglevel error -f lavfi -i testsrc=duration=2:size=320x240:rate=15 "$HOME/videos/[Studio] Someone - A Rather Long Title for a Video File That Wraps (2026) [720p].mp4"
gt_display
gt_arkha "$HOME/videos"; sleep 3; gt_quiet
gt_click 600 400; gt_key Home
gt_shot before
gt_key F2; sleep 1
gt_shot renaming
gt_key Escape; sleep 0.5
# The item's picture: the area above where its name (or its field) starts.
crop() { magick "$OUT/$1.png" -crop 140x52+190+112 +repage "$OUT/$1-icon.png"; }
crop before; crop renaming
diff=$(magick compare -metric AE "$OUT/before-icon.png" "$OUT/renaming-icon.png" null: 2>&1 | cut -d' ' -f1)
gt_check "the picture stays while renaming" "$([ "${diff%.*}" -lt 200 ] && echo same || echo "changed ($diff pixels)")" "same"
gt_end
