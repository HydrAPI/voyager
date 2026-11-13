#!/bin/sh
# build.sh: builds the prebuilt program for a release page, which the AUR's
# arkha-bin installs: target/tarball/arkha-<version>-<arch>.tar.gz with a
# folder arkha-<version>-<arch>/ holding arkha (it runs from there too, with
# its translations), arkha.desktop, arkha.svg, the AppStream metainfo
# (io.github.dekart811.Arkha.metainfo.xml), locale/, LICENSE and
# README.md. <version> is Cargo.toml's. Needs what building Arkha needs (see
# BUILDING.md).
set -e
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
arch=$(uname -m)
cargo build --release --locked

name=arkha-$version-$arch
out=target/tarball
rm -rf "$out"
mkdir -p "$out/$name"
install -m755 target/release/arkha "$out/$name/arkha"
install -m644 arkha.desktop LICENSE README.md "$out/$name/"
install -m644 data/arkha.svg "$out/$name/arkha.svg"
install -m644 data/io.github.dekart811.Arkha.metainfo.xml "$out/$name/"
# The translations build.rs made from po/*.po.
for mo in target/release/locale/*/LC_MESSAGES/arkha.mo; do
    [ -e "$mo" ] && install -Dm644 "$mo" "$out/$name/${mo#target/release/}"
done
# The same archive for the same files: sorted, with plain owners and times.
tar -C "$out" --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$(git log -1 --format=%ct 2>/dev/null || date +%s)" \
    -czf "$out/$name.tar.gz" "$name"
ls "$out/$name.tar.gz"
