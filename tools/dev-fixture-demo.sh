#!/usr/bin/env bash
# dev-fixture-demo.sh — spin up a throwaway tissue repo full of fixture issues
# (#171 scenarios; script itself tracked as #246).
#
# Usage (on the deployment box):
#   sudo bash dev-fixture-demo.sh [DIR] [PORT]
#     DIR  default /srv/tissue-test   (must not exist or be empty — the real
#                                      /srv/tissue-issues is never touched)
#     PORT default 7324               (GUI port; 7323 is the production serve)
#
# Run as root it re-execs itself as the unprivileged `tissue` user (never run
# fixtures as root/sudo directly — deploy/README.md, #226).
set -euo pipefail

DIR="${1:-/srv/tissue-test}"
PORT="${2:-7324}"
SERVICE_USER=tissue
TISSUE="${TISSUE_BIN:-$(command -v tissue || true)}"

if [ -z "$TISSUE" ]; then
  echo "error: tissue binary not found on PATH" >&2
  exit 1
fi

# Drop privileges: run everything as the service user, never root.
if [ "$(id -u)" = "0" ]; then
  exec sudo -u "$SERVICE_USER" -- env "TISSUE_BIN=$TISSUE" \
    PATH="$(dirname "$TISSUE"):/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    bash "$0" "$@"
fi

if [ -e "$DIR" ] && [ -n "$(ls -A "$DIR" 2>/dev/null)" ]; then
  echo "refusing: $DIR exists and is not empty." >&2
  echo "  if it is a disposable test repo: rm -rf '$DIR' and re-run" >&2
  exit 1
fi

mkdir -p "$DIR"
echo "== init throwaway repo at $DIR (project e2e, prefix IAM)"
"$TISSUE" init --dir "$DIR" --project e2e --id-prefix IAM

CFG="$DIR/.tissue/config.yml"
echo "== enable dev_fixtures + add second project (VER)"
grep -q '^dev_fixtures:' "$CFG" || sed -i '1i dev_fixtures: true' "$CFG"
"$TISSUE" --repo "$DIR" project add second VER

echo "== generate fixture issues (deterministic seeds)"
"$TISSUE" --repo "$DIR" dev-fixture IAM-9  --scenario human-edited          --seed 1
"$TISSUE" --repo "$DIR" dev-fixture IAM-10 --scenario agent-proposed        --seed 2
"$TISSUE" --repo "$DIR" dev-fixture IAM-11 --scenario both-changed          --seed 3
"$TISSUE" --repo "$DIR" dev-fixture IAM-12 --scenario agent-created-pending --seed 4
"$TISSUE" --repo "$DIR" dev-fixture IAM-13 --scenario synced-clean          --seed 5
"$TISSUE" --repo "$DIR" dev-fixture VER-1  --scenario agent-proposed        --seed 6

echo
echo "== done. repo: $DIR"
"$TISSUE" --repo "$DIR" list
echo
echo "GUI (loopback only):  $TISSUE --repo \"$DIR\" serve --port $PORT   →  http://127.0.0.1:$PORT"
echo "CLI:                   $TISSUE --repo \"$DIR\" list / show IAM-11 / sync IAM-9"
echo "wipe the test repo:    rm -rf \"$DIR\"   (production /srv/tissue-issues untouched)"
