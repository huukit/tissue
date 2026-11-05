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
#   TISSUE_UNIT_DIR    systemd unit dir (default: /etc/systemd/system); set to
#                      empty to skip unit refresh (#167)

set -euo pipefail

REPO_SLUG="${TISSUE_REPO_SLUG:-huukit/tissue}"
BIN_DIR="${TISSUE_BIN_DIR:-/usr/local/bin}"
ASSET_PATTERN="${TISSUE_ASSET:-tissue-v*-x86_64-unknown-linux-musl.tar.gz}"
BIN_PATH="${BIN_DIR}/tissue"
ROLLBACK_PATH="${BIN_DIR}/tissue.previous"
UNIT_DIR="${TISSUE_UNIT_DIR:-/etc/systemd/system}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
units_refreshed=0

log() { printf 'tissue-upgrade: %s\n' "$*" >&2; }

# --- 1. What is the latest release? ----------------------------------------
# Fetch the whole JSON body to a file first: piping curl into `grep -m1`
# makes grep exit after the first match, curl then gets EPIPE on the
# remaining body and exits 23 (CURLE_WRITE_ERROR), which `set -o pipefail`
# turns into a script failure (#130) once the release body exceeds the pipe
# buffer.
release_json="$WORK/release.json"
if ! curl -fsSL --max-time 30 -o "$release_json" \
    "https://api.github.com/repos/${REPO_SLUG}/releases/latest"; then
    log "could not fetch the latest release metadata"
    exit 1
fi
latest_tag="$(grep -m1 '"tag_name"' "$release_json" \
    | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
if [ -z "$latest_tag" ]; then
    log "could not determine the latest release tag"
    exit 1
fi

# Resolve the concrete asset from the release's asset list (#115: names
# are versioned, so nothing is hardcoded). The pattern is a shell glob,
# matched with `case` (NOT grep -x: in grep, `*` is a regex quantifier,
# not a glob wildcard, and the pattern can never match).
latest_assets="$(grep -o '"name": *"tissue-[^"]*"' "$release_json" \
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
# Post-swap sanity check (#128): never activate a binary that cannot run —
# if it fails, roll back immediately and keep the old version serving.
if ! "${BIN_PATH}.new" --version >/dev/null 2>&1; then
    log "new binary failed --version sanity check -- rolling back, keeping old version"
    rm -f "${BIN_PATH}.new"
    exit 1
fi
mv -f "${BIN_PATH}.new" "$BIN_PATH"
# Stamp the verified archive hash so the next run's currency check is a
# cheap sidecar-vs-stamp comparison (#88).
printf '%s  %s\n' "$expected_sha" "$ASSET" > "${BIN_PATH}.sha"
log "installed ${latest_tag} (rollback copy: ${ROLLBACK_PATH})"

# --- 4b. Unit-file staleness check (#167) -----------------------------------
# The binary auto-upgrades but unit files live in /etc and do NOT — a stale
# unit caused EROFS on the owner's box (v0.1.5: ProtectSystem=strict without
# ReadWritePaths, because the box ran the pre-#158 unit). Compare the
# installed units against the ones at the release tag (same GitHub repo,
# same trust boundary) and warn loudly with the exact remediation. If the
# script runs as root and the dir is writable, refresh in place.
if [ -n "$UNIT_DIR" ] && [ -d "$UNIT_DIR" ]; then
    for unit in tissue.service tissue-upgrade.service tissue-upgrade.timer; do
        installed="$UNIT_DIR/$unit"
        [ -f "$installed" ] || continue
        ref="$WORK/ref-$unit"
        if curl -fsSL --max-time 20 -o "$ref" \
            "https://raw.githubusercontent.com/${REPO_SLUG}/${latest_tag}/deploy/$unit" 2>/dev/null; then
            if ! cmp -s "$ref" "$installed"; then
                if [ -w "$UNIT_DIR" ]; then
                    install -m 0644 "$ref" "$installed"
                    log "refreshed stale unit $installed from ${REPO_SLUG}@${latest_tag} (#167)"
                    units_refreshed=1
                else
                    log "WARNING: installed $unit is STALE vs ${REPO_SLUG}@${latest_tag} (#167) — refresh with: sudo curl -fsSL 'https://raw.githubusercontent.com/${REPO_SLUG}/${latest_tag}/deploy/$unit' -o '$installed' && sudo systemctl daemon-reload"
                fi
            fi
        fi
        # Hard guard for the exact v0.1.5 failure mode: strict sandbox, no
        # writable path -> every API/GUI write fails EROFS.
        if [ "$unit" = "tissue.service" ] \
            && grep -q '^ProtectSystem=strict$' "$installed" \
            && ! grep -q '^ReadWritePaths=' "$installed"; then
            log "WARNING: $installed has ProtectSystem=strict but no ReadWritePaths — writes will fail EROFS (#167); add ReadWritePaths=/srv/tissue-issues and daemon-reload"
        fi
    done
fi

# --- 5. Restart the service so the new version actually runs ----------------
# (daemon-reload first when a unit changed on disk, #167)
if [ "$units_refreshed" = "1" ]; then
    systemctl daemon-reload || log "daemon-reload failed (not root?) — units take effect on next boot"
fi
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
