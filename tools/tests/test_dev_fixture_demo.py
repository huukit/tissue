#!/usr/bin/env python3
"""Static checks for tools/dev-fixture-demo.sh (#246).

The script needs a real `tissue` binary and a service user, so CI verifies
it statically: syntax, the safety guards, and the scenario coverage.
"""
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools" / "dev-fixture-demo.sh"

SCENARIOS = [
    "human-edited",
    "agent-proposed",
    "both-changed",
    "agent-created-pending",
    "synced-clean",
]


def check(cond: bool, msg: str) -> None:
    if not cond:
        print(f"FAIL: {msg}")
        sys.exit(1)
    print(f"ok: {msg}")


def main() -> None:
    check(SCRIPT.is_file(), "tools/dev-fixture-demo.sh exists")
    text = SCRIPT.read_text()

    r = subprocess.run(["bash", "-n", str(SCRIPT)], capture_output=True, text=True)
    check(r.returncode == 0, f"bash -n clean ({r.stderr.strip() or 'no output'})")

    check("dev_fixtures: true" in text, "inserts the #171 dev_fixtures opt-in guard")
    check(re.search(r"id -u.*=.*0", text) is not None, "drops privileges when run as root")
    check("SERVICE_USER=tissue" in text, "re-execs as the unprivileged tissue user")
    check('chown "$SERVICE_USER:$SERVICE_USER" "$DIR"' in text,
          "creates the target dir while root and chowns it (parents like /srv are root-owned)")
    check("deploy/install.sh first" in text, "fails with a clear hint when the tissue user is missing (#132)")
    check(text.index("refusing: $DIR exists") < text.index('mkdir -p "$DIR"'),
          "non-empty-dir refusal happens before any mkdir")
    check("exists and is not empty" in text, "refuses a non-empty target dir")
    check('DIR="${1:-/srv/tissue-test}"' in text, "default target dir is the throwaway /srv/tissue-test")
    check(re.search(r"^\s*DIR=.*tissue-issues\b", text, re.M) is None,
          "never assigns the production /srv/tissue-issues as the target dir")
    for s in SCENARIOS:
        check(f"--scenario {s}" in text, f"generates scenario {s}")
    check("--seed" in text, "deterministic seeds")
    check("serve --port" in text, "prints a loopback serve hint with the port")
    check("project add second VER" in text, "adds the second project (VER)")

    print("ALL DEV-FIXTURE-DEMO TESTS PASSED")


if __name__ == "__main__":
    main()
