#!/bin/bash
# tissue-upgrade: poll the latest GitHub Release, verify, and swap the binary.
#
# Designed to run from the tissue-upgrade systemd timer (see
# tissue-upgrade.service/.timer). Boring by design: bash + curl + sha256sum.
#
# Safety rules (issue #83):
#   - downloads only from the repo's GitHub Releases
#   - verifies the .sha256 sidecar BEFORE swapping; failed verify = no swap
#   - keeps exactly one previous binary for rollback
#   - never restarts the service unless a new binary actually landed
#
# Environment (may be overridden in tissue-upgrade.service):
#   TISSUE_REPO_SLUG   GitHub owner/repo (default: huukit/tissue)
#   TISSUE_BIN_DIR     install dir (default: /usr/local/bin)
#   TISSUE_ASSET       release asset name (default: tissue-x86_64-unknown-linux-musl.tar.gz)

set -euo pipefail

REPO_SLUG="${TISSUE_REPO_SLUG:-huukit/tissue}"
BIN_DIR="${TISSUE_BIN_DIR:-/usr/local/bin}"
ASSET="${TISSUE_ASSET:-tissue-x86_64-unknown-linux-musl.tar.gz}"
BIN_PATH="${BIN_DIR}/tissue"
ROLLBACK_PATH="${BIN_DIR}/tissue.previous"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

log() { printf 'tissue-upgrade: %s\n' "$*" >&2; }

# --- 1. What is the latest release? ----------------------------------------
latest_tag="$(curl -fsSL --max-time 30 \
    "https://api.github.com/repos/${REPO_SLUG}/releases/latest" \
    | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
if [ -z "$latest_tag" ]; then
    log "could not determine the latest release tag"
    exit 1
fi

# Content-based currency check (#88): the on-disk binary's sha256 vs the
# published asset sidecar. `--version` is NOT trusted — published binaries
# have carried a stale version string before (v0.1.1 reported 0.1.0).
base="https://github.com/${REPO_SLUG}/releases/download/${latest_tag}"
expected_sha="$(curl -fsSL --max-time 30 "$base/$ASSET.sha256" \
    | awk '{print $1}')"
if [ -z "$expected_sha" ]; then
    log "could not fetch the sha256 sidecar for ${latest_tag}"
    exit 1
fi
if [ -x "$BIN_PATH" ] && sha256sum "$BIN_PATH" 2>/dev/null | grep -q "^${expected_sha} "; then
    log "installed binary already matches ${latest_tag}; nothing to do"
    exit 0
fi
log "installed binary differs from ${latest_tag}; upgrading"

# --- 2. Download the asset -------------------------------------------------
if ! curl -fsSL --max-time 120 -o "$WORK/$ASSET" "$base/$ASSET"; then
    log "download failed for $ASSET"
    exit 1
fi

# --- 3. Verify BEFORE any swap ----------------------------------------------
cd "$WORK"
# The downloaded asset must match the sidecar fetched in step 1 (guard
# against a sidecar swap between the two fetches).
printf '%s  %s\n' "$expected_sha" "$ASSET" > "$WORK/expected.sha256"
if ! sha256sum -c "$WORK/expected.sha256" >/dev/null 2>&1; then
    log "sha256 verification FAILED for $ASSET — no swap, service keeps the old version"
    exit 1
fi
log "sha256 verified"

tar xzf "$ASSET"
new_bin="$(find . -type f -name tissue | head -1)"
if [ -z "$new_bin" ]; then
    log "archive did not contain a tissue binary"
    exit 1
fi
chmod 0755 "$new_bin"

# --- 4. Atomic-ish swap with one rollback copy ------------------------------
if [ -x "$BIN_PATH" ]; then
    cp -f "$BIN_PATH" "$ROLLBACK_PATH"
fi
install -m 0755 "$new_bin" "${BIN_PATH}.new"
mv -f "${BIN_PATH}.new" "$BIN_PATH"
log "installed ${latest_tag} (rollback copy: ${ROLLBACK_PATH})"

# --- 5. Restart the service so the new version actually runs ----------------
if systemctl is-active --quiet tissue.service; then
    if ! systemctl restart tissue.service; then
        log "service restart FAILED — rollback with: systemctl stop tissue.service && cp -f ${ROLLBACK_PATH} ${BIN_PATH} && systemctl start tissue.service"
        exit 1
    fi
    sleep 1
    if ! systemctl is-active --quiet tissue.service; then
        log "service failed to come up after restart — roll back with: cp -f ${ROLLBACK_PATH} ${BIN_PATH} && systemctl restart tissue.service"
        exit 1
    fi
    log "tissue.service restarted and active on the new version"
else
    log "tissue.service not active; binary swapped, start it manually"
fi
