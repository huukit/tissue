#!/usr/bin/env python3
"""Inspect the process table for runaways and zombies (#228).

Developer sessions in constrained sandboxes exhaust the process table:
unreaped zombie children (state `Z` -- only their parent can reap them) left
behind by the test battery or by killed sessions accumulate until `fork()`
fails with EAGAIN ("Cannot fork"), which then takes down the whole session.

This tool reports the table state and is read-only by default:

    tools/procwatch.py                          # human report, exit 1 if zombies
    tools/procwatch.py --json                   # machine-readable
    tools/procwatch.py --allow-zombies          # exit 0 even with zombies
    tools/procwatch.py --kill-orphans           # SIGKILL live orphans (ppid 1)
    tools/procwatch.py --kill-orphans --watch git tissue
    tools/procwatch.py --proc-root /tmp/fakeproc  # test against a fake /proc

`--kill-orphans` only ever targets *live* processes whose parent is gone
(ppid 1 or ppid not in the table) and whose comm matches `--watch`
(default: git, tissue). Zombies cannot be killed -- they are already dead;
only their parent can reap them, so the report shows the parent pid. The
tool never targets itself or pid 1.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
from dataclasses import dataclass, field

DEFAULT_WATCH = ["git", "tissue"]


@dataclass
class Proc:
    pid: int
    ppid: int
    state: str
    comm: str
    starttime: int


@dataclass
class ProcTable:
    proc_root: str
    procs: dict[int, Proc] = field(default_factory=dict)
    pid_max: int = 0
    uptime: float = 0.0

    @property
    def total(self) -> int:
        return len(self.procs)

    @property
    def zombies(self) -> list[Proc]:
        return [p for p in self.procs.values() if p.state == "Z"]

    def headroom_pct(self) -> float:
        if self.pid_max <= 0:
            return 0.0
        return max(0.0, 100.0 * (self.pid_max - self.total) / self.pid_max)

    def age(self, p: Proc, clk_tck: int) -> float:
        if self.uptime <= 0 or clk_tck <= 0:
            return -1.0
        return max(0.0, self.uptime - p.starttime / clk_tck)


def _parse_stat(pid_dir: str) -> Proc | None:
    """Parse /proc/<pid>/stat; comm may contain spaces/parens, so split after
    the LAST ')'. Fields: 1 pid, 2 (comm), 3 state, 4 ppid, ... 22 starttime."""
    try:
        with open(os.path.join(pid_dir, "stat"), "r", encoding="utf-8") as f:
            raw = f.read()
    except OSError:
        return None
    rparen = raw.rfind(")")
    if rparen < 0:
        return None
    comm = raw[raw.find("(") + 1 : rparen]
    rest = raw[rparen + 1 :].split()
    if len(rest) < 20:
        return None
    try:
        return Proc(
            pid=int(raw[: raw.find("(")].strip()),
            ppid=int(rest[1]),
            state=rest[0],
            comm=comm,
            starttime=int(rest[19]),
        )
    except ValueError:
        return None


def _read_int(path: str, default: int = 0) -> int:
    try:
        with open(path, "r", encoding="utf-8") as f:
            return int(f.read().strip())
    except (OSError, ValueError):
        return default


def snapshot(proc_root: str = "/proc") -> ProcTable:
    table = ProcTable(proc_root=proc_root)
    table.pid_max = _read_int(os.path.join(proc_root, "sys", "kernel", "pid_max"), 4194304)
    try:
        with open(os.path.join(proc_root, "uptime"), "r", encoding="utf-8") as f:
            table.uptime = float(f.read().split()[0])
    except (OSError, ValueError, IndexError):
        table.uptime = 0.0
    try:
        entries = os.listdir(proc_root)
    except OSError:
        return table
    for name in entries:
        if not name.isdigit():
            continue
        p = _parse_stat(os.path.join(proc_root, name))
        if p is not None:
            table.procs[p.pid] = p
    return table


def is_orphan(table: ProcTable, p: Proc) -> bool:
    """Parent is gone: reparented to pid 1 (or the reaper), which in a
    non-reaping sandbox is exactly where zombies come from."""
    return p.pid != 1 and (p.ppid == 1 or p.ppid not in table.procs)


def kill_orphans(
    table: ProcTable,
    comms: list[str],
    killer=os.kill,
    self_pid: int | None = None,
) -> list[int]:
    """SIGKILL live orphans matching `comms`. Never touches zombies (already
    dead), non-orphans, pid 1, or this process itself."""
    self_pid = os.getpid() if self_pid is None else self_pid
    killed: list[int] = []
    for p in sorted(table.procs.values(), key=lambda p: p.pid):
        if p.state == "Z" or p.pid in (1, self_pid):
            continue
        if p.comm in comms and is_orphan(table, p):
            try:
                killer(p.pid, signal.SIGKILL)
                killed.append(p.pid)
            except OSError:
                pass
    return killed


def comm_counts(table: ProcTable) -> dict[str, int]:
    counts: dict[str, int] = {}
    for p in table.procs.values():
        counts[p.comm] = counts.get(p.comm, 0) + 1
    return counts


def report(table: ProcTable, watch: list[str], clk_tck: int) -> dict:
    zombies = table.zombies
    watched = [
        {
            "pid": p.pid,
            "ppid": p.ppid,
            "comm": p.comm,
            "state": p.state,
            "age_sec": round(table.age(p, clk_tck), 1),
        }
        for p in sorted(table.procs.values(), key=lambda p: p.pid)
        if p.comm in watch and p.state != "Z"
    ]
    return {
        "proc_root": table.proc_root,
        "total": table.total,
        "pid_max": table.pid_max,
        "headroom_pct": round(table.headroom_pct(), 2),
        "zombies": [
            {"pid": z.pid, "ppid": z.ppid, "comm": z.comm} for z in zombies
        ],
        "zombie_count": len(zombies),
        "top_comms": dict(
            sorted(comm_counts(table).items(), key=lambda kv: -kv[1])[:10]
        ),
        "watched_live": watched,
    }


def print_report(rep: dict, killed: list[int] | None) -> None:
    print(
        f"process table: {rep['total']}/{rep['pid_max']} entries "
        f"({100.0 - rep['headroom_pct']:.1f}% used, headroom {rep['headroom_pct']:.1f}%) "
        f"[proc_root={rep['proc_root']}]"
    )
    zc = rep["zombie_count"]
    print(f"zombies: {zc}" + ("  <-- parent is not reaping; only the parent can reap" if zc else ""))
    for z in rep["zombies"]:
        print(f"  pid {z['pid']} ppid {z['ppid']} comm {z['comm']} <defunct>")
    top = ", ".join(f"{c}({n})" for c, n in rep["top_comms"].items())
    print(f"top comms: {top}")
    print(f"watched live processes: {len(rep['watched_live'])}")
    for w in rep["watched_live"]:
        age = f"{w['age_sec']}s" if w["age_sec"] >= 0 else "?"
        orphan = " orphan" if w["ppid"] == 1 else ""
        print(
            f"  pid {w['pid']} ppid {w['ppid']} comm {w['comm']} "
            f"state {w['state']} age {age}{orphan}"
        )
    if killed is not None:
        print(f"killed orphans: {killed if killed else 'none'}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description="Inspect the process table for zombies/runaways (#228). "
        "Read-only unless --kill-orphans is given."
    )
    ap.add_argument("--proc-root", default="/proc", help="procfs mount (default /proc)")
    ap.add_argument("--json", action="store_true", help="machine-readable report")
    ap.add_argument(
        "--watch",
        nargs="+",
        default=DEFAULT_WATCH,
        help=f"comms to list/kill (default: {' '.join(DEFAULT_WATCH)})",
    )
    ap.add_argument(
        "--kill-orphans",
        action="store_true",
        help="SIGKILL live orphaned (ppid 1) processes whose comm is in --watch",
    )
    ap.add_argument(
        "--allow-zombies",
        action="store_true",
        help="exit 0 even when zombies are present",
    )
    args = ap.parse_args(argv)

    table = snapshot(args.proc_root)
    clk_tck = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
    rep = report(table, args.watch, clk_tck)

    killed: list[int] | None = None
    if args.kill_orphans:
        killed = kill_orphans(table, args.watch)
        rep = report(table, args.watch, clk_tck)
        rep["killed_orphans"] = killed

    if args.json:
        print(json.dumps(rep, indent=2))
    else:
        print_report(rep, killed)

    if rep["zombie_count"] > 0 and not args.allow_zombies:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
