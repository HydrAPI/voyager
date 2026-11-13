#!/bin/sh
# version.sh FORMAT [VERSION]: the version for a package format (deb or
# rpm), from VERSION as remote-build.sh writes it (default: the file
# VERSION, else Cargo.toml's version): base[-pre][+build], such as
# 1.0.0-dev+r142.g6a7d287. A build towards a release (with -pre) sorts
# before it: 1.0.0~dev.r142.g6a7d287 in both; one after a release after
# it: 1.0.0+r150.g… (deb), 1.0.0^r150.g… (rpm); a release is 1.0.0.
cd "$(dirname "$0")/.."
format=${1:?usage: version.sh deb|rpm [VERSION]}
v=${2:-$(cat VERSION 2>/dev/null || sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)}
build=
case $v in *+*) build=${v#*+}; v=${v%%+*} ;; esac
base=${v%%-*}
pre=
[ "$v" != "$base" ] && pre=${v#*-}
out=$base
[ -n "$pre" ] && out="$out~$pre"
if [ -n "$build" ]; then
    if [ -n "$pre" ]; then
        out="$out.$build"
    elif [ "$format" = deb ]; then
        out="$out+$build"
    else
        out="$out^$build"
    fi
fi
printf '%s\n' "$out"
