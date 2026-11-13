#!/bin/sh
# remote-build.sh HOST SCRIPT RESULTS OUT: builds a package on HOST, a
# machine reached as `ssh HOST`, for deb/build-on.sh and rpm/build-on.sh:
# copies this checkout's files (tracked and new ones, not the ignored) with
# the crates (cargo vendor, so the build needs no internet) there with
# rsync, into ~/arkha-build, runs SCRIPT there (a path in the checkout),
# and fetches RESULTS (a pattern in ~/arkha-build) into OUT. The version,
# in the file VERSION, is the PKGBUILD's: 0.1.0+r<commits>.g<commit>,
# ".dirty" with uncommitted changes. HOST keeps its build folder between
# runs, so rebuilds are quicker.
set -e
[ $# -eq 4 ] || { echo "usage: remote-build.sh HOST SCRIPT RESULTS OUT" >&2; exit 2; }
host=$1 script=$2 results=$3 out=$4
cd "$(dirname "$0")/.."
st=target/remote-src
rm -rf "$st"
mkdir -p "$st/.cargo"
git ls-files -co --exclude-standard | rsync -a --files-from=- ./ "$st/"
cargo vendor --locked --quiet "$st/vendor" >/dev/null
cat > "$st/.cargo/config.toml" <<'EOF'
# The crates come with the sources (cargo vendor): built without internet.
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)+r$(git rev-list --count HEAD).g$(git rev-parse --short HEAD)"
# The PKGBUILD's version line is always changed by makepkg: not counted.
[ -n "$(git status --porcelain --untracked-files=no -- . ':!packaging/arch/PKGBUILD')" ] && version="$version.dirty"
echo "$version" > "$st/VERSION"

rsync -az --delete --exclude=/target "$st/" "$host:arkha-build/"
ssh "$host" "sh arkha-build/$script"
rsync -a "$host:arkha-build/$results" "$out/"
# shellcheck disable=SC2086
ls -l "$out"/$(basename "$results")
