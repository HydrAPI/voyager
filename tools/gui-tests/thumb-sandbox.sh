#!/bin/bash
# thumb-sandbox.sh x11|wayland: thumbnailers in the bubblewrap sandbox
# make the same thumbnails as without it, for each of the sample files
# (videos, PDF, PostScript, office documents, audio, fonts, pictures…);
# and where bwrap can't make a sandbox (here: one that fails as a system
# that doesn't allow them makes it fail), thumbnails are made all the
# same, without one, and that's noted.
GT_CONF='show_thumbnails=always'
. "$(dirname "$0")/lib.sh"
"$GT_DIR/fixtures.sh" bench >/dev/null || exit 2
gt_session "$@"

cp -r "$GT_FIX/thumbtest" "$HOME/samples"
# A bwrap that fails as it does where user namespaces aren't allowed.
mkdir -p "$HOME/nosandbox"
printf '#!/bin/sh\necho "bwrap: setting up uid map: Permission denied" >&2\nexit 1\n' > "$HOME/nosandbox/bwrap"
chmod +x "$HOME/nosandbox/bwrap"
export G_MESSAGES_DEBUG=voyager
# The files that got a thumbnail (their thumbnails' names), one per line.
made() { find "$HOME/.cache/thumbnails" -name '*.png' -not -path '*/fail/*' -printf '%f\n' 2>/dev/null | sort; }
round() {  # round NAME [ENV...]: Voyager in the samples folder until it's quiet
  rm -rf "$HOME/.cache/thumbnails"; : > "$OUT/voyager.log"
  env "${@:2}" "$VOYAGER" "$HOME/samples" >> "$OUT/voyager.log" 2>&1 & VOYAGER_PID=$!
  sleep 8; gt_quiet; gt_shot "$1"
  made > "$OUT/$1.list"
  # (The refused round's log apart from the check for warnings: the fake
  # bwrap makes glycin, the system's image loader, warn that it runs
  # without its own sandbox.)
  if [ "$1" = refused ]; then mv "$OUT/voyager.log" "$OUT/refused.log"; : > "$OUT/voyager.log"; else cp "$OUT/voyager.log" "$OUT/voyager-$1.log"; fi
  gt_quit
}
gt_display
round sandbox
round plain VOYAGER_THUMBNAIL_SANDBOX=0
round refused PATH="$HOME/nosandbox:$PATH"
n() { wc -l < "$OUT/$1.list"; }
gt_note "thumbnails: $(n sandbox) in the sandbox, $(n plain) without, $(n refused) where it's refused"
gt_check "thumbnailers ran in the sandbox" "$(grep -c 'runs in a sandbox' "$OUT/voyager-sandbox.log" | grep -qv '^0$' && echo yes || echo no)" "yes"
gt_check "the same thumbnails with the sandbox as without" "$(diff -q "$OUT/sandbox.list" "$OUT/plain.list" >/dev/null && echo same || echo different)" "same"
gt_check "most samples got one" "$([ "$(n sandbox)" -ge 20 ] && echo yes || echo "only $(n sandbox)")" "yes"
gt_check "refused: the same thumbnails, without a sandbox" "$(diff -q "$OUT/refused.list" "$OUT/plain.list" >/dev/null && echo same || echo different) $(grep -c 'runs in a sandbox' "$OUT/refused.log")" "same 0"
gt_check "refused: noted" "$(grep -c "bwrap can't make a sandbox here" "$OUT/refused.log")" "1"
gt_end
