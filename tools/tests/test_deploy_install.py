#!/usr/bin/env python3
"""Static checks for deploy/install.sh (#132).

The script needs a real systemd box, so CI verifies it statically: syntax,
idempotency guards, migration path, and path defaults matching the units.
"""
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "deploy" / "install.sh"
UNIT = (REPO / "deploy" / "tissue.service").read_text()


def check(cond: bool, msg: str) -> None:
    if not cond:
        print(f"FAIL: {msg}")
        sys.exit(1)
    print(f"ok: {msg}")


def main() -> None:
    check(SCRIPT.is_file(), "deploy/install.sh exists")
    text = SCRIPT.read_text()

    r = subprocess.run(["bash", "-n", str(SCRIPT)], capture_output=True, text=True)
    check(r.returncode == 0, f"bash -n clean ({r.stderr.strip() or 'no output'})")

    check('id "$SERVICE_USER" >/dev/null 2>&1' in text, "user creation is idempotent (guards on existing user)")
    check("useradd --system" in text and "nologin" in text, "creates a system user with nologin shell")
    check('chown -R "$SERVICE_USER:$SERVICE_USER" "$REPO_DIR"' in text,
          "migration path: chown -R existing repo to the service user")
    check(re.search(r"ls -A .*REPO_DIR.*\)\"\]", text) is not None or "already initialized" in text,
          "never re-initializes an existing repo")
    check('install -m 0644 "$SRC_DIR/tissue.service"' in text, "installs unit files")
    check("/etc/sudoers.d/tissue" in text and "0440" in text, "writes narrow sudoers entry for systemctl control")
    check("systemctl enable --now tissue.service" in text and "tissue-upgrade.timer" in text,
          "enables viewer service + upgrade timer")
    check("systemctl is-active tissue.service" in text, "verifies the service is active")
    check("127.0.0.1:7323" in text, "verifies loopback binding")
    check("--from-github" in text, "supports --from-github OWNER/REPO init")
    check('sudo -u "$SERVICE_USER"' in text, "repo init runs as the service user, never root")
    check(text.index("no binary in $BIN_DIR") < text.index("tissue init --dir"),
          "binary is installed BEFORE repo init (init needs the binary; box hit this)")
    check(re.search(r'curl -fsSL "https://api\.github\.com/repos/[^"]*releases/latest" -o ', text) is not None,
          "release JSON is fetched to a file before grep (no curl|grep -m1 EPIPE under pipefail, #130)")
    check(re.search(r"curl[^|]*\|\s*grep -m1", text) is None,
          "no curl|grep -m1 pipeline anywhere in the script")
    check('ln -sfn "$BIN_DIR/tissue" /usr/local/bin/tissue' in text,
          "symlinks /usr/local/bin/tissue to the timer-managed binary (replaces old root install)")

    # defaults must match the literal ExecStart paths in the unit (#129)
    check('TISSUE_REPO_DIR:-/srv/tissue-issues' in text and "--repo /srv/tissue-issues" in UNIT,
          "default repo dir matches tissue.service ExecStart")
    check('TISSUE_BIN_DIR:-/opt/tissue/bin' in text and "/opt/tissue/bin/tissue" in UNIT,
          "default bin dir matches tissue.service ExecStart binary path")

    print("ALL DEPLOY-INSTALL TESTS PASSED")


if __name__ == "__main__":
    main()
