#!/bin/sh
# alpha.sh ON_WHITE ON_BLACK OUT: the same part of the screen taken on a
# white and on a black backdrop, made into one picture with real
# transparency (the window's rounded corners and shadow), cut to what's
# visible: alpha = 1 - (white - black), colour = black / alpha.
set -e
w=$1 b=$2 out=$3
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
magick "$w" "$b" -compose difference -composite -colorspace gray -negate "$tmp/alpha.png"
magick "$b" "$tmp/alpha.png" -fx 'v.r > 0 ? u / v.r : 0' "$tmp/color.png"
magick "$tmp/color.png" "$tmp/alpha.png" -alpha off -compose CopyOpacity -composite -trim +repage "$out"
