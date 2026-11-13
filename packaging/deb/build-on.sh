#!/bin/sh
# build-on.sh HOST: builds the .deb on HOST, a Debian 13+ or Ubuntu 26.04+
# machine reached as `ssh HOST` with the build packages (see build.sh),
# and fetches it into packaging/deb/ (see ../remote-build.sh).
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../remote-build.sh" "${1:?usage: build-on.sh HOST}" packaging/deb/build.sh 'target/deb/*.deb' "$here"
