#!/bin/sh
# build-on.sh HOST: builds the .deb on HOST, a Debian 13+ or Ubuntu 26.04+
# machine reached as `ssh HOST` with the build packages (see build.sh):
# copies this checkout's files (tracked and new ones, not the ignored) with
# the crates (cargo vendor) there with rsync, into ~/arkha-build, runs
# build.sh, and fetches the package into packaging/deb/. The version is
# the PKGBUILD's (0.1.0+r<commits>.g<commit>), ".dirty" with uncommitted
# changes. HOST keeps its build folder between runs: rebuilds are quicker.
set -e
host=${1:?usage: build-on.sh HOST}
cd "$(dirname "$0")/../.."
st=target/deb-src
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
ssh "$host" 'sh arkha-build/packaging/deb/build.sh'
rsync -a "$host:arkha-build/target/deb/*.deb" packaging/deb/
ls -l packaging/deb/*.deb
