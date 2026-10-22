#!/usr/bin/env python3
"""sample.py <pid> <seconds> <label> — once a second, the PSS of <pid> and
every process under it (WebKit's helpers count, as in mem-bench.sh), and
their CPU over the whole window. Prints one line: the label, PSS at the
end, the peak, threads, and CPU as % of one core. Linux only (/proc)."""
import os, sys, time

pid, seconds, label = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
tck = os.sysconf("SC_CLK_TCK")


def tree(root):
    kids = {}
    for p in os.listdir("/proc"):
        if not p.isdigit():
            continue
        try:
            stat = open(f"/proc/{p}/stat").read()
        except OSError:
            continue
        ppid = int(stat.rsplit(")", 1)[1].split()[1])
        kids.setdefault(ppid, []).append(int(p))
    out, todo = [], [root]
    while todo:
        p = todo.pop()
        out.append(p)
        todo += kids.get(p, [])
    return out


def pss_kb(p):
    try:
        for line in open(f"/proc/{p}/smaps_rollup"):
            if line.startswith("Pss:"):
                return int(line.split()[1])
    except OSError:
        pass
    return 0


def ticks(p):
    try:
        f = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()
        return int(f[11]) + int(f[12])
    except OSError:
        return 0


def threads(p):
    try:
        return len(os.listdir(f"/proc/{p}/task"))
    except OSError:
        return 0


# CPU per pid, so a helper that exits mid-window is not counted negative.
seen, first = {}, {}
peak = last = 0
for _ in range(seconds):
    procs = tree(pid)
    for p in procs:
        t = ticks(p)
        first.setdefault(p, t)
        seen[p] = t
    last = sum(pss_kb(p) for p in procs)
    peak = max(peak, last)
    time.sleep(1)
procs = tree(pid)
cpu = sum(seen[p] - first[p] for p in seen) / tck / seconds * 100
thr = sum(threads(p) for p in procs)
print(f"{label:<34} {last/1024:7.1f} MB  peak {peak/1024:7.1f} MB  {thr:4d} threads  {cpu:6.2f} % cpu")
