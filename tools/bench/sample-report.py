#!/usr/bin/env python3
"""sample-report.py OUT: what a sample.sh profile spent its time in, the
samples where the main thread was waiting (polling) left out: Voyager's
innermost function in each sample, Voyager's functions counting every sample
they were on the stack for, and the innermost function of all."""
import collections
import sys

stacks = [line.rstrip("\n").split(";") for line in open(sys.argv[1])]
idle = lambda st: any(x in ("poll", "__poll", "ppoll", "g_main_context_poll", "g_main_context_poll_unlocked") for x in st[:6])
busy = [st for st in stacks if not idle(st)]
print(f"samples {len(stacks)}, busy {len(busy)}")
inclusive, innermost, first = collections.Counter(), collections.Counter(), collections.Counter()
for st in busy:
    for n in set(st):
        inclusive[n] += 1
    innermost[st[0]] += 1
    first[next((n for n in st if "voyager::" in n), "<no voyager frame>")] += 1


def show(title, counts, k):
    print(f"--- {title}")
    for n, v in counts.most_common(k):
        print(f"{v:5d} {100 * v / max(len(busy), 1):5.1f}%  {n[:150]}")


show("innermost Voyager function", first, 30)
show("Voyager functions, inclusive", collections.Counter({n: v for n, v in inclusive.items() if "voyager::" in n}), 40)
show("innermost function", innermost, 25)
