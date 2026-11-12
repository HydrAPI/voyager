# Sampling profiler for the main thread, gdb's part (see sample.sh):
# it writes the inferior's pid to $PIDFILE, then on every SIGUSR2 stop records
# the main thread's stack (innermost first, ';'-separated) to $OUT, until the
# file $STOPFILE exists.
import gdb, os

OUT = os.environ["OUT"]
PIDFILE = os.environ["PIDFILE"]
STOPFILE = os.environ["STOPFILE"]
for c in ("set pagination off", "set print thread-events off", "set print inferior-events off",
          "handle SIGUSR2 stop print nopass", "handle SIGPIPE nostop noprint pass"):
    gdb.execute(c)

gdb.execute("starti", to_string=True)
with open(PIDFILE + ".tmp", "w") as f:
    f.write(str(gdb.selected_inferior().pid))
os.rename(PIDFILE + ".tmp", PIDFILE)
out = open(OUT, "w")
while not os.path.exists(STOPFILE):
    try:
        gdb.execute("continue", to_string=True)
    except gdb.error as e:
        print("continue failed:", e)
        break
    if not gdb.selected_inferior().pid or os.path.exists(STOPFILE):
        break
    try:
        main = min(gdb.selected_inferior().threads(), key=lambda t: t.num)
        main.switch()
        f = gdb.newest_frame()
        names = []
        while f is not None and len(names) < 60:
            names.append(f.name() or "??")
            f = f.older()
        out.write(";".join(names) + "\n")
    except gdb.error:
        pass
out.close()
try:
    gdb.execute("kill", to_string=True)
except gdb.error:
    pass
