# Testing-machine deployment

One binary, one box, zero infrastructure. `tissue serve` runs as a systemd
service bound to loopback (127.0.0.1) as a dedicated unprivileged `tissue`
user (#132) — remote access stays **SSH-tunnel-only** (the viewer is
read-only; the tunnel preserves the loopback-only model). A systemd timer
polls the GitHub Releases for a new tag, sha256-verifies the download, and
swaps the binary with one rollback copy. It follows `releases/latest`, which
is the latest **published** (non-draft) release — releases are published
automatically when their tag is pushed; if a tag ever lingers as a draft it
is skipped, so upgrades always track a published release.

This is the only document you need for one-time setup.

## Files

| File | Purpose |
|---|---|
| `tissue.service` | the viewer: `tissue serve` on 127.0.0.1 as user `tissue`, repo/port in the literal `ExecStart` |
| `tissue-upgrade.service` | oneshot: poll latest Release → verify → swap → restart (as user `tissue`) |
| `tissue-upgrade.timer` | runs the upgrade every 15 minutes |
| `tissue-upgrade.sh` | the upgrade logic (bash + curl + sha256sum only) |

## One-time setup (on the testing box, as root)

1. **Create the dedicated user and the directories** (#132). The binary
   lives in `/opt/tissue/bin` so `/usr/local/bin` stays root-owned; the
   issue repo lives in the user's home:

   ```sh
   useradd --system --home /srv/tissue-issues --shell /usr/sbin/nologin tissue
   mkdir -p /opt/tissue/bin /usr/local/lib/tissue
   chown tissue:tissue /opt/tissue/bin /srv/tissue-issues
   ```

2. **Install the files**:

   ```sh
   cp tissue.service tissue-upgrade.service tissue-upgrade.timer /etc/systemd/system/
   cp tissue-upgrade.sh /usr/local/lib/tissue/
   chmod 0755 /usr/local/lib/tissue/tissue-upgrade.sh
   ```

3. **Initialize the issue repo as the tissue user** (so it owns the files):

   ```sh
   sudo -u tissue tissue init --dir /srv/tissue-issues        # empty repo
   # or: sudo -u tissue tissue init --from-github OWNER/REPO --dir /srv/tissue-issues
   ```

4. **Configure** the viewer for your box: edit the literal paths in the
   `ExecStart=` line of `/etc/systemd/system/tissue.service` (systemd does
   not expand `$VARIABLES` from `Environment=` inside `ExecStart=` — using
   them there fails with 203/EXEC, #129/#128):

   - `--repo` — the directory containing your issue repo's `.tissue/`
     (default in the unit: `/srv/tissue-issues`; this is your *data*, not a
     code checkout)
   - `--port` — viewer port (default `7323`)
   - the binary path is fixed at `/opt/tissue/bin/tissue` (managed by
     tissue-upgrade via `TISSUE_BIN_DIR`; change both together if you move it)

   The unit also sets the write-API token — never commit a real one; use an
   `EnvironmentFile=` (mode 0600) instead:

   ```sh
   printf 'TISSUE_API_TOKEN=%s\n' "$(openssl rand -hex 24)" > /etc/tissue/env
   chmod 0600 /etc/tissue/env && chown root:tissue /etc/tissue/env
   # then in tissue.service: EnvironmentFile=/etc/tissue/env
   ```

   With no token set, serve starts read-only and prints a startup note (#120).

5. **Install the current binary** once so the service can start. Resolve the
   exact versioned asset name from the latest published release first (the
   archive names carry the version, e.g. `tissue-v0.1.4-x86_64-unknown-linux-musl.tar.gz`):

   ```sh
   asset="$(curl -fsSL https://api.github.com/repos/huukit/tissue/releases/latest \
     | grep -m1 '"name": "tissue-v.*x86_64-unknown-linux-musl.tar.gz"$' \
     | sed -E 's/.*"name": "([^"]+)".*/\1/')"
   curl -fsSL -o "/tmp/$asset" \
     "https://github.com/huukit/tissue/releases/latest/download/$asset"
   curl -fsSL -o "/tmp/$asset.sha256" \
     "https://github.com/huukit/tissue/releases/latest/download/$asset.sha256"
   cd /tmp && sha256sum -c "$asset.sha256"
   tar xzf "$asset"
   install -m 0755 -o tissue -g tissue tissue /opt/tissue/bin/tissue
   # #213: install the man page too (the auto-upgrade script only swaps the
   # binary, so this is a one-time step; `man tissue` then works)
   install -d /usr/local/share/man/man1
   install -m 0644 share/man/man1/tissue.1 /usr/local/share/man/man1/tissue.1
   ```

6. **Let the upgrade user control the viewer service.** The upgrade runs as
   `tissue` and must be allowed to restart `tissue.service` (#132). Either a
   polkit rule or a narrow sudoers entry:

   ```sh
   cat > /etc/sudoers.d/tissue <<'EOF'
   tissue ALL=(root) NOPASSWD: /usr/bin/systemctl restart tissue.service, /usr/bin/systemctl is-active tissue.service, /usr/bin/systemctl stop tissue.service, /usr/bin/systemctl start tissue.service
   EOF
   chmod 0440 /etc/sudoers.d/tissue
   ```

   (The upgrade script calls plain `systemctl`; with this entry it works as
   the `tissue` user. Polkit is the alternative; pick one.)

7. **Verify the write path under the service sandbox** (#167 — a stale unit
   with `ProtectSystem=strict` and no `ReadWritePaths` makes every GUI/API
   write fail with EROFS):

   ```sh
   sudo -u tissue test -w /srv/tissue-issues && echo "write path OK"
   systemctl cat tissue.service | grep -E 'ProtectSystem|ReadWritePaths'
   ```

   **Upgrade the unit files on every release, not just the binary** (#167):
   the auto-upgrade timer swaps `/opt/tissue/bin/tissue` but unit files live
   in `/etc/systemd/system` and only change when you copy them. After each
   release (or when the upgrade log prints a `STALE unit` warning):

   ```sh
   for u in tissue.service tissue-upgrade.service tissue-upgrade.timer; do
     curl -fsSL "https://raw.githubusercontent.com/huukit/tissue/vX.Y.Z/deploy/$u" \
       | sudo tee "/etc/systemd/system/$u" >/dev/null
   done
   sudo systemctl daemon-reload && sudo systemctl restart tissue.service
   ```

8. **Enable**:

   ```sh
   systemctl daemon-reload
   systemctl enable --now tissue.service
   systemctl enable --now tissue-upgrade.timer
   ```

8. **View from your workstation** over the SSH tunnel (loopback stays
   loopback on the box):

   ```sh
   ssh -L 7323:127.0.0.1:7323 user@testing-box
   # then open http://127.0.0.1:7323 locally
   ```

From now on: publish a new Release → the box picks it up within 15 minutes,
verifies, swaps, and restarts. **One manual step per release:** the timer
upgrades the *binary* only — compare the unit files with this repo's
`deploy/` at the new tag and refresh if they differ (#167; the upgrade
script logs a `STALE unit` / EROFS warning when it detects the mismatch):

```sh
for u in tissue.service tissue-upgrade.service tissue-upgrade.timer; do
  curl -fsSL "https://raw.githubusercontent.com/huukit/tissue/vX.Y.Z/deploy/$u" \
    | sudo tee "/etc/systemd/system/$u" >/dev/null
  sudo systemctl cat "$u" | cmp -s - <(curl -fsSL "https://raw.githubusercontent.com/huukit/tissue/vX.Y.Z/deploy/$u") || echo "$u changed"
done
sudo systemctl daemon-reload && sudo systemctl restart tissue.service
sudo -u tissue test -w /srv/tissue-issues && echo "write path OK"
```

## First start fails? (#131)

`tissue.service` exits 1 at startup for exactly two reasons — both are
visible in the journal:

```sh
journalctl -u tissue.service -n 30
```

- **`not a tissue repository (no .tissue/ in <path>)`** — the `--repo` path
  is wrong or the repo was never initialized there. The unit's
  `ExecStartPre=/usr/bin/test -d <repo>/.tissue` guard also fails at unit
  level with a clearer message. Fix: point `--repo` at the directory that
  contains `.tissue/` (e.g. `/srv/tissue-issues`).
- **`binding 127.0.0.1:7323: address already in use`** — a stale manually
  started `tissue serve` still holds the port. Find and stop it:

  ```sh
  ss -ltnp | grep 7323
  ```

(203/EXEC after an upgrade, the earlier failure mode, is fixed: `ExecStart`
uses literal paths only, and the upgrade script refuses to activate a binary
that fails `--version`, rolling back instead — #128.)

- **`Read-only file system (os error 30)` / EROFS on any write (GUI "+ New
  issue", API POST)** — the installed unit has `ProtectSystem=strict` but no
  `ReadWritePaths=` (#167): the binary auto-upgraded, the unit file didn't.
  Check with `systemctl cat tissue.service | grep -E 'ProtectSystem|ReadWritePaths'`,
  then refresh the unit files per step 7 above, `daemon-reload`, restart.
  Probe the write path as the service user: `sudo -u tissue test -w /srv/tissue-issues`.
  The upgrade script now also logs a `STALE unit` / EROFS warning when it
  detects this mismatch.

## Rollback (one-liner)

If a new version misbehaves, the previous binary is kept at
`/opt/tissue/bin/tissue.previous`:

```sh
cp -f /opt/tissue/bin/tissue.previous /opt/tissue/bin/tissue && systemctl restart tissue.service
```

To roll back the service-user change (#132): remove the `User=`/`Group=`
lines from both units, `systemctl daemon-reload`, and restart.

## Uninstall

```sh
systemctl disable --now tissue.service tissue-upgrade.timer
rm -f /etc/systemd/system/tissue.service /etc/systemd/system/tissue-upgrade.service /etc/systemd/system/tissue-upgrade.timer
rm -rf /opt/tissue /srv/tissue-issues /usr/local/lib/tissue
rm -f /usr/local/bin/tissue /usr/local/share/man/man1/tissue.1   # #213 man page
systemctl daemon-reload
```

## Safety properties

- Downloads only from `huukit/tissue` GitHub Releases (no other sources)
- `sha256sum -c` against the published `.sha256` sidecar **before** any swap;
  failed verification = no swap, the service keeps running the old version
- Post-install `--version` sanity check: a broken binary is never activated
  (rolled back instead) — #128
- Currency check is content-based, not version-string-based: the upgrade
  script stamps the verified archive hash in `tissue.sha` next to the binary
  and no-ops when the latest Release's sidecar matches the stamp
- One rollback copy (`tissue.previous`) maintained by the upgrade script
- No restart unless a new binary actually installed; if the service fails to
  come up after a restart, the log line prints the exact rollback command
- Runs as the unprivileged `tissue` user; `/usr/local/bin` stays root-owned
  (binary dir is `/opt/tissue/bin`) — #132
- Loopback-only: the service binds 127.0.0.1; remote access is SSH-tunnel-only
- No secrets in tracked files: the API token lives in an untracked
  `EnvironmentFile`, never in the unit or repo
