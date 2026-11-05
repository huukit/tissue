import json, os, subprocess, sys, tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "tools"))
import procwatch


def stat_line(pid, comm, state, ppid, starttime=100):
    # fields after "(comm)": 3=state, 4=ppid, 5..21 filler, 22=starttime
    rest = " ".join([state, str(ppid)] + ["0"] * 17 + [str(starttime)])
    return f"{pid} ({comm}) {rest}\n"


def make_fake_proc(root, procs, pid_max=4194304, uptime="1000.00 500.00"):
    root = Path(root)
    (root / "sys" / "kernel").mkdir(parents=True, exist_ok=True)
    (root / "sys" / "kernel" / "pid_max").write_text(str(pid_max))
    (root / "uptime").write_text(uptime + "\n")
    for pid, comm, state, ppid in procs:
        d = root / str(pid)
        d.mkdir(exist_ok=True)
        (d / "stat").write_text(stat_line(pid, comm, state, ppid))
    return str(root)


# --- unit: snapshot parses stat, finds zombies and pid_max ---
root = make_fake_proc(
    tempfile.mkdtemp(),
    [
        (1, "init", "S", 0),
        (100, "git", "Z", 1),   # zombie, parent = pid 1 (non-reaping sandbox init)
        (112, "git", "Z", 1),   # second zombie
        (200, "git", "S", 1),   # live orphan
        (201, "git", "S", 200), # live child of a live process: NOT an orphan
        (202, "tissue", "R", 1),
        (203, "sh", "S", 202),
    ],
)
t = procwatch.snapshot(root)
assert t.total == 7, t.total
assert t.pid_max == 4194304
z = t.zombies
assert [p.pid for p in z] == [100, 112], z
assert all(p.ppid == 1 for p in z)
assert abs(t.headroom_pct() - 100.0 * (4194304 - 7) / 4194304) < 0.01
print("snapshot/zombies ok")

# --- unit: orphan detection ---
assert procwatch.is_orphan(t, t.procs[100])
assert procwatch.is_orphan(t, t.procs[200])
assert not procwatch.is_orphan(t, t.procs[201])
assert not procwatch.is_orphan(t, t.procs[1])  # pid 1 is never an orphan
print("is_orphan ok")

# --- unit: kill_orphans only kills live, matching, orphaned pids ---
killed_calls = []
killed = procwatch.kill_orphans(
    t, ["git", "tissue"], killer=lambda pid, sig: killed_calls.append((pid, sig)),
    self_pid=203,
)
assert killed == [200, 202], killed  # 100/112 are zombies (un-killable), 201 not orphan, 203 is self
assert all(sig == 9 for _, sig in killed_calls)
print("kill_orphans ok:", killed)

# --- unit: report shape + zombie exit code ---
rep = procwatch.report(t, ["git", "tissue"], clk_tck=100)
assert rep["zombie_count"] == 2
assert rep["total"] == 7 and rep["pid_max"] == 4194304
assert {w["pid"] for w in rep["watched_live"]} == {200, 201, 202}
json.dumps(rep)  # must be JSON-serializable
rc = procwatch.main(["--proc-root", root, "--json"])
assert rc == 1, rc  # zombies present -> exit 1
rc = procwatch.main(["--proc-root", root, "--json", "--allow-zombies"])
assert rc == 0, rc
print("report/exit-code ok")

# --- unit: clean tree exits 0 ---
clean = make_fake_proc(tempfile.mkdtemp(), [(1, "init", "S", 0), (50, "sh", "S", 1)])
rc = procwatch.main(["--proc-root", clean])
assert rc == 0, rc
print("clean tree ok")

# --- unit: kill-orphans via CLI only touches the fake tree's orphans ---
root2 = make_fake_proc(
    tempfile.mkdtemp(),
    [(1, "init", "S", 0), (300, "git", "S", 1), (301, "bash", "S", 1)],
)
rc = procwatch.main(["--proc-root", root2, "--kill-orphans", "--allow-zombies"])
assert rc == 0
print("cli kill-orphans ok (no real processes touched)")

# --- smoke: runs against the real /proc and prints a report ---
out = subprocess.run(
    [sys.executable, str(REPO_ROOT / "tools" / "procwatch.py"), "--allow-zombies"],
    capture_output=True, text=True,
)
assert out.returncode == 0, out.stderr
assert "process table:" in out.stdout and "zombies:" in out.stdout
print("real /proc smoke ok")
print("ALL PROCWATCH TESTS PASSED")
