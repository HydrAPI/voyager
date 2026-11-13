#!/bin/sh
# build-on.sh HOST: builds the RPM on HOST, a Fedora machine reached as
# `ssh HOST` with the build packages (see build.sh), and fetches it into
# packaging/rpm/ (see ../remote-build.sh).
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../remote-build.sh" "${1:?usage: build-on.sh HOST}" packaging/rpm/build.sh 'target/rpm/*.rpm' "$here"
