#!/bin/sh
# build.sh: builds Arkha's RPM, on Fedora: run in a copy of the sources made
# by build-on.sh (the crates in vendor/, built without internet; the
# version in VERSION), or in a checkout with internet for the crates.
# Needs cargo and rust 1.85+, gcc, gtk3-devel, gettext and rpm-build. The
# package goes to target/rpm/.
set -e
cd "$(dirname "$0")/../.."
top=$(pwd)
rpmver=$(sh packaging/version.sh rpm)
rm -rf target/rpm
mkdir -p target/rpm
if [ -d vendor ]; then
    rpmbuild -bb --build-in-place --define "_topdir $top/target/rpm" --define "arkha_version $rpmver" \
        --define "arkha_offline --offline" packaging/rpm/arkha.spec
else
    rpmbuild -bb --build-in-place --define "_topdir $top/target/rpm" --define "arkha_version $rpmver" \
        packaging/rpm/arkha.spec
fi
mv target/rpm/RPMS/*/arkha-*.rpm target/rpm/
ls target/rpm/*.rpm
