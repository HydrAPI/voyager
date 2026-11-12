#!/bin/bash
# fixtures.sh base|bench: makes what the GUI tests and benchmarks need, in
# target/gui-fixtures (once; delete a folder there to make it again).
#   base   share-noapps/: /usr/share without applications (as XDG_DATA_DIRS,
#            Arkha finds no program to open a file with, and starts none);
#          xmonad/: xmonad with Super as its modifier (Alt stays Arkha's).
#   bench  big20k/: 20,000 empty files; thumbtest/: one file of each of 30
#          or so types that get thumbnails (made with ImageMagick, ffmpeg and
#          Python; types a missing tool would make are left out).
set -e
FIX=$(cd "$(dirname "$0")/../.." && pwd)/target/gui-fixtures
mkdir -p "$FIX"

base() {
  if [ ! -d "$FIX/share-noapps" ]; then
    mkdir -p "$FIX/share-noapps.new"
    for e in /usr/share/*; do [ "$(basename "$e")" = applications ] || ln -s "$e" "$FIX/share-noapps.new/"; done
    mv "$FIX/share-noapps.new" "$FIX/share-noapps"
  fi
  # Built again when a system update took away a library it was linked with.
  if { [ ! -x "$FIX/xmonad/xmonad-x86_64-linux" ] && [ ! -e "$FIX/xmonad/xmonad.ok" ]; } \
     || ldd "$FIX/xmonad/xmonad-x86_64-linux" 2>/dev/null | grep -q 'not found'; then
    mkdir -p "$FIX/xmonad"
    printf 'import XMonad\nmain :: IO ()\nmain = xmonad def { modMask = mod4Mask }\n' > "$FIX/xmonad/xmonad.hs"
    XMONAD_CONFIG_DIR=$FIX/xmonad XMONAD_CACHE_DIR=$FIX/xmonad XMONAD_DATA_DIR=$FIX/xmonad xmonad --recompile
    touch "$FIX/xmonad/xmonad.ok"
  fi
}

bench() {
  if [ ! -d "$FIX/big20k" ]; then
    mkdir -p "$FIX/big20k.new"
    (cd "$FIX/big20k.new" && for i in $(seq 10000 29999); do : > "file $i.txt"; done)
    mv "$FIX/big20k.new" "$FIX/big20k"
  fi
  if [ ! -d "$FIX/thumbtest" ]; then
    local t=$FIX/thumbtest.new
    rm -rf "$t"; mkdir -p "$t"
    if command -v magick >/dev/null; then
      magick -size 640x400 plasma:fractal -seed 7 "$t/base.png"
      for ext in png jpg gif bmp tif webp tga xpm ico avif heic jxl; do
        magick "$t/base.png" -resize "$([ $ext = ico ] && echo 64x64 || echo 640x400)" "$t/sample.$ext" 2>/dev/null || rm -f "$t/sample.$ext"
      done
      magick "$t/base.png" "$t/sample.pdf" 2>/dev/null || true
      magick "$t/base.png" "$t/sample.ps" 2>/dev/null || true
    fi
    printf '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="120"><rect width="200" height="120" fill="#2f6db5"/><circle cx="100" cy="60" r="40" fill="#f6f7f8"/></svg>\n' > "$t/sample.svg"
    if command -v ffmpeg >/dev/null; then
      local f="ffmpeg -loglevel error -y"
      $f -f lavfi -i testsrc=duration=2:size=320x240:rate=15 "$t/sample.mp4"
      $f -f lavfi -i testsrc=duration=2:size=320x240:rate=15 "$t/sample.mkv"
      $f -f lavfi -i testsrc=duration=2:size=320x240:rate=15 -c:v libvpx "$t/sample.webm" 2>/dev/null || rm -f "$t/sample.webm"
      # Audio with cover art (what audio thumbnailers show).
      $f -f lavfi -i sine=duration=2 -i "$t/base.png" -map 0 -map 1 -c:v mjpeg -disposition:v attached_pic "$t/sample.mp3" 2>/dev/null || rm -f "$t/sample.mp3"
      $f -f lavfi -i sine=duration=2 -i "$t/base.png" -map 0 -map 1 -c:v png -disposition:v attached_pic "$t/sample.flac" 2>/dev/null || rm -f "$t/sample.flac"
      $f -f lavfi -i sine=duration=2 "$t/sample.ogg" 2>/dev/null || rm -f "$t/sample.ogg"
      $f -f lavfi -i sine=duration=2 "$t/sample.wav"
    fi
    # Office documents with the thumbnail pictures office thumbnailers read.
    python3 - "$t" <<'EOF'
import sys, zipfile, os
t = sys.argv[1]
png = open(os.path.join(t, "base.png"), "rb").read() if os.path.exists(os.path.join(t, "base.png")) else None
for ext, mime in (("odt", "text"), ("ods", "spreadsheet"), ("odp", "presentation")):
    with zipfile.ZipFile(os.path.join(t, "sample." + ext), "w") as z:
        z.writestr(zipfile.ZipInfo("mimetype"), "application/vnd.oasis.opendocument." + mime)
        z.writestr("META-INF/manifest.xml", '<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.%s"/></manifest:manifest>' % mime)
        z.writestr("content.xml", '<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"/>')
        if png:
            z.writestr("Thumbnails/thumbnail.png", png)
for ext, main in (("docx", "word/document.xml"), ("xlsx", "xl/workbook.xml"), ("pptx", "ppt/presentation.xml")):
    with zipfile.ZipFile(os.path.join(t, "sample." + ext), "w") as z:
        z.writestr("[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="png" ContentType="image/png"/><Default Extension="xml" ContentType="application/xml"/></Types>')
        z.writestr("_rels/.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" Target="docProps/thumbnail.png"/></Relationships>')
        z.writestr(main, '<?xml version="1.0"?><root/>')
        if png:
            z.writestr("docProps/thumbnail.png", png)
EOF
    for f in $(fc-list : file | grep -iE '\.ttf:|\.otf:' | head -40 | cut -d: -f1); do
      case "$f" in *.ttf) [ -e "$t/sample.ttf" ] || cp "$f" "$t/sample.ttf" ;; *.otf) [ -e "$t/sample.otf" ] || cp "$f" "$t/sample.otf" ;; esac
    done
    echo "Plain text, for text previews in icons." > "$t/sample.txt"
    printf '<!doctype html><title>Sample</title><p>Sample page.</p>\n' > "$t/sample.html"
    rm -f "$t/base.png"
    mv "$t" "$FIX/thumbtest"
  fi
}

case "${1:-base}" in
  base) base ;;
  bench) base; bench ;;
  *) echo "usage: $0 base|bench" >&2; exit 2 ;;
esac
echo "$FIX"
