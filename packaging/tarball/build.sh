#!/bin/sh
# build.sh: builds the prebuilt program for a release page, which the AUR's
# voyager-bin installs: target/tarball/voyager-<version>-<arch>.tar.gz with a
# folder voyager-<version>-<arch>/ holding voyager (it runs from there too, with
# its translations), voyager.desktop, voyager.svg, the AppStream metainfo
# (io.github.HydrAPI.Voyager.metainfo.xml), locale/, LICENSE and
# README.md. <version> is Cargo.toml's. Needs what building Voyager needs (see
# BUILDING.md).
set -e
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
arch=$(uname -m)
cargo build --release --locked

name=voyager-$version-$arch
out=target/tarball
rm -rf "$out"
mkdir -p "$out/$name"
install -m755 target/release/voyager "$out/$name/voyager"
install -m644 voyager.desktop LICENSE README.md "$out/$name/"
install -m644 data/voyager.svg "$out/$name/voyager.svg"
install -m644 data/io.github.HydrAPI.Voyager.metainfo.xml "$out/$name/"
# The translations build.rs made from po/*.po.
for mo in target/release/locale/*/LC_MESSAGES/voyager.mo; do
    [ -e "$mo" ] && install -Dm644 "$mo" "$out/$name/${mo#target/release/}"
done
# Never a tarball without them (as the first 1.0.0 one was): what's
# there instead, to see why.
if [ ! -d "$out/$name/locale" ]; then
    echo "build.sh: no translations in target/release/locale" >&2
    echo "msgfmt: $(command -v msgfmt || echo none) $(msgfmt --version 2>/dev/null | head -n1)" >&2
    ls -laR target/release/locale >&2 || true
    for f in target/release/build/voyager-*/out/languages.rs; do echo "$f:" >&2; cat "$f" >&2; done
    exit 1
fi
# The same archive for the same files: sorted, with plain owners and times.
tar -C "$out" --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$(git log -1 --format=%ct 2>/dev/null || date +%s)" \
    -czf "$out/$name.tar.gz" "$name"
ls "$out/$name.tar.gz"
