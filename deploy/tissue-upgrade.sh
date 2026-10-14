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
#   TISSUE_ASSET       asset name pattern (default: tissue-v*-x86_64-unknown-linux-musl.tar.gz;
#                      resolved against the release's asset list, so it is version-agnostic)

set -euo pipefail

REPO_SLUG="${TISSUE_REPO_SLUG:-huukit/tissue}"
BIN_DIR="${TISSUE_BIN_DIR:-/usr/local/bin}"
ASSET_PATTERN="${TISSUE_ASSET:-tissue-v*-x86_64-unknown-linux-musl.tar.gz}"
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

# Resolve the concrete asset from the release's asset list (#115: names
# are versioned, so nothing is hardcoded). The pattern is a shell glob,
# matched with `case` (NOT grep -x: in grep, `*` is a regex quantifier,
# not a glob wildcard, and the pattern can never match).
latest_assets="$(curl -fsSL --max-time 30 \
    "https://api.github.com/repos/${REPO_SLUG}/releases/latest" \
    | grep -o '"name": *"tissue-[^"]*"' \
    | sed -E 's/.*"name": *"([^"]+)".*/\1/')"
ASSET=""
# shellcheck disable=SC2254 # $ASSET_PATTERN is unquoted on purpose: the
# pattern is a user-overridable glob, and case-branch globs are how it
# matches versioned asset names.
while IFS= read -r a; do
    case "$a" in
        $ASSET_PATTERN) ASSET="$a"; break ;;
    esac
done <<EOF2
$latest_assets
EOF2
if [ -z "$ASSET" ]; then
    log "no asset matching ${ASSET_PATTERN} in the latest release"
    exit 1
fi
log "resolved asset: ${ASSET}"

# Content-based currency check (#88): `--version` is NOT trusted — published
# binaries have carried a stale version string before (v0.1.1 reported 0.1.0).
# The sidecar hashes the release ARCHIVE, not the extracted binary, so the
# on-disk binary cannot be compared to it directly. Instead, at swap time we
# stamp the verified archive hash next to the binary (${BIN_PATH}.sha); the
# cheap no-op path is: fetch sidecar -> compare stamp -> exit 0.
base="https://github.com/${REPO_SLUG}/releases/download/${latest_tag}"
expected_sha="$(curl -fsSL --max-time 30 "$base/$ASSET.sha256" \
    | awk '{print $1}')"
if [ -z "$expected_sha" ]; then
    log "could not fetch the sha256 sidecar for ${latest_tag}"
    exit 1
fi
if [ -x "$BIN_PATH" ] && [ -f "${BIN_PATH}.sha" ] \
    && grep -q "^${expected_sha}" "${BIN_PATH}.sha"; then
    log "installed binary already matches ${latest_tag}; nothing to do"
    exit 0
fi
log "installed binary differs from ${latest_tag} (or no stamp); upgrading"

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
# Stamp the verified archive hash so the next run's currency check is a
# cheap sidecar-vs-stamp comparison (#88).
printf '%s  %s\n' "$expected_sha" "$ASSET" > "${BIN_PATH}.sha"
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
