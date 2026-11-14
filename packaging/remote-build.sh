#!/bin/sh
# remote-build.sh HOST SCRIPT RESULTS OUT: builds a package on HOST, a
# machine reached as `ssh HOST`, for deb/build-on.sh and rpm/build-on.sh:
# copies this checkout's files (tracked and new ones, not the ignored) with
# the crates (cargo vendor, so the build needs no internet) there with
# rsync, into ~/voyager-build, runs SCRIPT there (a path in the checkout),
# and fetches RESULTS (a pattern in ~/voyager-build) into OUT. The file
# VERSION has the version for the packages, Cargo.toml's with the build:
# "1.0.0-dev+r142.g6a7d287" (".dirty" with uncommitted changes), or just
# "1.0.0" for a release (the commit tagged v1.0.0); deb/build.sh and
# rpm/build.sh write it the way their package formats sort it. HOST keeps
# its build folder between runs, so rebuilds are quicker.
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
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
# The PKGBUILD's version line is always changed by makepkg: not counted.
dirty=
[ -n "$(git status --porcelain --untracked-files=no -- . ':!packaging/arch/PKGBUILD')" ] && dirty=.dirty
if [ -n "$dirty" ] || ! git describe --exact-match --tags --match "v$version" >/dev/null 2>&1; then
    version="$version+r$(git rev-list --count HEAD).g$(git rev-parse --short HEAD)$dirty"
fi
echo "$version" > "$st/VERSION"

rsync -az --delete --exclude=/target "$st/" "$host:voyager-build/"
ssh "$host" "sh voyager-build/$script"
rsync -a "$host:voyager-build/$results" "$out/"
# shellcheck disable=SC2086
ls -l "$out"/$(basename "$results")
