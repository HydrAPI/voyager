#!/bin/bash
# sample.sh SECONDS OUT BINARY ARGS...: a profile without perf (which the
# kernel may not allow): runs BINARY under gdb and records its main
# thread's stack every ~10 ms for SECONDS into OUT, one line each,
# innermost frame first. SKIP=n waits n seconds first (to measure an
# action rather than the start). Summarize with sample-report.py OUT.
# Use a build with symbols:
#   CARGO_TARGET_DIR=target/prof-build CARGO_PROFILE_RELEASE_DEBUG=1 \
#     CARGO_PROFILE_RELEASE_STRIP=false cargo build --release
# Run it inside a test session (see tools/gui-tests/lib.sh) or a private
# X server: it starts the program like any other.
B=$(cd "$(dirname "$0")" && pwd)
[ $# -ge 3 ] || { echo "usage: $0 SECONDS OUT BINARY ARGS..." >&2; exit 2; }
secs=$1 out=$2; shift 2
tmp=$(mktemp -d)
export OUT=$out PIDFILE=$tmp/pid STOPFILE=$tmp/stop
gdb -q -batch -x "$B/sample.py" --args "$@" > "$tmp/gdb.out" 2>&1 &
g=$!
for _ in $(seq 200); do [ -s "$PIDFILE" ] && break; sleep 0.05; done
pid=$(cat "$PIDFILE")
sleep "${SKIP:-0}"
end=$(( $(date +%s%N) + secs * 1000000000 ))
while [ "$(date +%s%N)" -lt "$end" ] && kill -0 "$pid" 2>/dev/null; do
  kill -USR2 "$pid" 2>/dev/null
  sleep 0.01
done
touch "$STOPFILE"
kill -USR2 "$pid" 2>/dev/null
wait $g
rm -rf "$tmp"
