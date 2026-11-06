#!/usr/bin/env bash
# install.sh — one-time testing-box setup for tissue (#132).
#
# Idempotent and migration-safe: safe on a fresh box AND on a box that was
# set up before the dedicated service user existed (it creates the `tissue`
# user, chowns the existing repo to it, and swaps in the hardened units —
# it never deletes or re-initializes an existing repo).
#
# Usage (as root):
#   sudo bash deploy/install.sh [--from-github OWNER/REPO]
#
# Env overrides: TISSUE_REPO_DIR (default /srv/tissue-issues),
#                TISSUE_BIN_DIR  (default /opt/tissue/bin)
#   — must match the literal ExecStart paths in tissue.service (#129:
#     systemd does not expand $VARIABLES inside ExecStart=).
set -euo pipefail

REPO_DIR="${TISSUE_REPO_DIR:-/srv/tissue-issues}"
BIN_DIR="${TISSUE_BIN_DIR:-/opt/tissue/bin}"
LIB_DIR=/usr/local/lib/tissue
UNIT_DIR=/etc/systemd/system
SERVICE_USER=tissue
FROM_GITHUB=""

while [ $# -gt 0 ]; do
  case "$1" in
    --from-github) FROM_GITHUB="${2:?--from-github needs OWNER/REPO}"; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1 (see --help)" >&2; exit 2 ;;
  esac
done

if [ "$(id -u)" != "0" ]; then
  echo "error: run as root: sudo bash $0 $*" >&2
  exit 1
fi

SRC_DIR="$(cd "$(dirname "$0")" && pwd)"
log() { printf 'install: %s\n' "$*"; }

# --- 1. dedicated unprivileged system user (#132) ---------------------------
if id "$SERVICE_USER" >/dev/null 2>&1; then
  log "user $SERVICE_USER already exists"
else
  useradd --system --home "$REPO_DIR" --shell /usr/sbin/nologin "$SERVICE_USER"
  log "created system user $SERVICE_USER (home $REPO_DIR, nologin)"
fi

# --- 2. directories + ownership ---------------------------------------------
mkdir -p "$BIN_DIR" "$LIB_DIR" "$REPO_DIR"
chown "$SERVICE_USER:$SERVICE_USER" "$BIN_DIR"
# Migration case: the repo was created by root before the user existed.
if [ -n "$(ls -A "$REPO_DIR" 2>/dev/null)" ]; then
  log "existing content in $REPO_DIR — chown -R to $SERVICE_USER (no re-init)"
  chown -R "$SERVICE_USER:$SERVICE_USER" "$REPO_DIR"
else
  chown "$SERVICE_USER:$SERVICE_USER" "$REPO_DIR"
fi

# --- 3. install unit files + upgrade script ----------------------------------
if systemctl is-active --quiet tissue.service 2>/dev/null; then
  log "stopping currently running tissue.service (will restart as $SERVICE_USER)"
  systemctl stop tissue.service || true
fi
install -m 0644 "$SRC_DIR/tissue.service" "$SRC_DIR/tissue-upgrade.service" "$SRC_DIR/tissue-upgrade.timer" "$UNIT_DIR/"
install -m 0755 "$SRC_DIR/tissue-upgrade.sh" "$LIB_DIR/tissue-upgrade.sh"
log "installed units to $UNIT_DIR and $LIB_DIR/tissue-upgrade.sh"

# --- 4. binary FIRST (repo init below needs it): install latest published
#        release only if missing ---------------------------------------------
if [ -x "$BIN_DIR/tissue" ]; then
  log "binary present at $BIN_DIR/tissue — keeping it (upgrade timer manages it)"
else
  log "no binary in $BIN_DIR — downloading latest published release"
  asset="$(curl -fsSL "https://api.github.com/repos/huukit/tissue/releases/latest" \
    | grep -m1 '"name": "tissue-v.*x86_64-unknown-linux-musl.tar.gz"$' \
    | sed -E 's/.*"name": "([^"]+)".*/\1/')"
  curl -fsSL -o "/tmp/$asset" "https://github.com/huukit/tissue/releases/latest/download/$asset"
  curl -fsSL -o "/tmp/$asset.sha256" "https://github.com/huukit/tissue/releases/latest/download/$asset.sha256"
  (cd /tmp && sha256sum -c "$asset.sha256")
  (cd /tmp && tar xzf "$asset")
  install -m 0755 -o "$SERVICE_USER" -g "$SERVICE_USER" /tmp/tissue "$BIN_DIR/tissue"
  # #213: man page is a one-time install (the upgrade timer swaps only the binary)
  if [ -f /tmp/share/man/man1/tissue.1 ]; then
    install -d /usr/local/share/man/man1
    install -m 0644 /tmp/share/man/man1/tissue.1 /usr/local/share/man/man1/tissue.1
  fi
  rm -f "/tmp/$asset" "/tmp/$asset.sha256" /tmp/tissue
  rm -rf /tmp/share
  log "binary installed to $BIN_DIR/tissue"
fi
# fall back to a system-installed tissue (e.g. old /usr/local/bin install) if
# the download was skipped or the unit path differs
TISSUE_BIN="$BIN_DIR/tissue"
if [ ! -x "$TISSUE_BIN" ]; then
  TISSUE_BIN="$(command -v tissue || true)"
fi
if [ -z "$TISSUE_BIN" ] || [ ! -x "$TISSUE_BIN" ]; then
  echo "error: no tissue binary in $BIN_DIR and none on PATH — cannot init the repo" >&2
  exit 1
fi

# --- 5. issue repo: init only if empty (as the service user, never root) -----
if [ -z "$(ls -A "$REPO_DIR" 2>/dev/null)" ]; then
  if [ -n "$FROM_GITHUB" ]; then
    log "tissue init --from-github $FROM_GITHUB --dir $REPO_DIR (as $SERVICE_USER)"
    sudo -u "$SERVICE_USER" -- env PATH="$BIN_DIR:/usr/local/bin:/usr/bin:/bin" \
      "$TISSUE_BIN" init --from-github "$FROM_GITHUB" --dir "$REPO_DIR"
  else
    log "tissue init --dir $REPO_DIR (as $SERVICE_USER)"
    sudo -u "$SERVICE_USER" -- env PATH="$BIN_DIR:/usr/local/bin:/usr/bin:/bin" \
      "$TISSUE_BIN" init --dir "$REPO_DIR"
  fi
else
  log "$REPO_DIR already initialized — skipping init"
fi

# --- 6. let the upgrade user control the viewer service (#132) ---------------
if [ ! -f /etc/sudoers.d/tissue ]; then
  printf '%s\n' \
    'tissue ALL=(root) NOPASSWD: /usr/bin/systemctl restart tissue.service, /usr/bin/systemctl is-active tissue.service, /usr/bin/systemctl stop tissue.service, /usr/bin/systemctl start tissue.service' \
    > /etc/sudoers.d/tissue
  chmod 0440 /etc/sudoers.d/tissue
  log "wrote /etc/sudoers.d/tissue (systemctl control for $SERVICE_USER)"
fi

# --- 7. enable + start --------------------------------------------------------
systemctl daemon-reload
systemctl enable --now tissue.service
systemctl enable --now tissue-upgrade.timer

# --- 8. verify ----------------------------------------------------------------
sudo -u "$SERVICE_USER" test -w "$REPO_DIR" && log "write path OK: $SERVICE_USER can write $REPO_DIR"
systemctl is-active tissue.service
ss -ltn | grep -q '127.0.0.1:7323' && log "viewer bound to loopback 127.0.0.1:7323" || log "WARN: 7323 not bound yet — check: journalctl -u tissue.service -n 30"
log "done. View from your workstation over SSH tunnel:"
log "  ssh -L 7323:127.0.0.1:7323 user@this-box   →   http://127.0.0.1:7323"
