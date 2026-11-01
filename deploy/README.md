# Testing-machine deployment

One binary, one box, zero infrastructure. `tissue serve` runs as a systemd
service bound to loopback (127.0.0.1) — remote access stays **SSH-tunnel-only**
(the viewer is read-only; the tunnel preserves the loopback-only model).
A systemd timer polls the GitHub Releases for a new tag, sha256-verifies the
download, and swaps the binary with one rollback copy. It follows
`releases/latest`, which is the latest **published** (non-draft) release —
releases are published automatically when their tag is pushed; if a tag ever
lingers as a draft it is skipped, so upgrades always track a published
release.

This is the only document you need for one-time setup.

## Files

| File | Purpose |
|---|---|
| `tissue.service` | the viewer: `tissue serve` on 127.0.0.1, env-configurable repo/port |
| `tissue-upgrade.service` | oneshot: poll latest Release → verify → swap → restart |
| `tissue-upgrade.timer` | runs the upgrade every 15 minutes |
| `tissue-upgrade.sh` | the upgrade logic (bash + curl + sha256sum only) |

## One-time setup (on the testing box)

1. **Install the files** (as root):

   ```sh
   mkdir -p /usr/local/lib/tissue
   cp tissue.service tissue-upgrade.service tissue-upgrade.timer /etc/systemd/system/
   cp tissue-upgrade.sh /usr/local/lib/tissue/
   chmod 0755 /usr/local/lib/tissue/tissue-upgrade.sh
   ```

2. **Configure** the viewer for your box: edit the literal paths in the
   `ExecStart=` line of `/etc/systemd/system/tissue.service` (systemd does
   not expand `$VARIABLES` from `Environment=` inside `ExecStart=` — using
   them there fails with 203/EXEC, #129):

   - `--repo` — the directory containing your issue repo's `.tissue/`
     (e.g. `/srv/tissue-issues`; this is your *data*, not a code checkout)
   - `--port` — viewer port (default `7323`)
   - the binary path is fixed at `/usr/local/bin/tissue` (managed by
     tissue-upgrade; override only via its `TISSUE_BIN_DIR` if you also
     change the ExecStart path to match)

3. **Install the current binary** once so the service can start. Resolve the
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
   tar xzf "$asset" && install -m 0755 tissue /usr/local/bin/tissue
   ```

4. **Enable**:

   ```sh
   systemctl daemon-reload
   systemctl enable --now tissue.service
   systemctl enable --now tissue-upgrade.timer
   ```

5. **View from your workstation** over the SSH tunnel (loopback stays
   loopback on the box):

   ```sh
   ssh -L 7323:127.0.0.1:7323 user@testing-box
   # then open http://127.0.0.1:7323 locally
   ```

From now on: publish a new Release → the box picks it up within 15 minutes,
verifies, swaps, and restarts. No manual steps.

## Rollback (one-liner)

If a new version misbehaves, the previous binary is kept at
`/usr/local/bin/tissue.previous`:

```sh
cp -f /usr/local/bin/tissue.previous /usr/local/bin/tissue && systemctl restart tissue.service
```

## Safety properties

- Downloads only from `huukit/tissue` GitHub Releases (no other sources)
- `sha256sum -c` against the published `.sha256` sidecar **before** any swap;
  failed verification = no swap, the service keeps running the old version
- Currency check is content-based, not version-string-based: the upgrade
  script stamps the verified archive hash in `tissue.sha` next to the binary
  and no-ops when the latest Release's sidecar matches the stamp
- One rollback copy (`tissue.previous`) maintained by the upgrade script
- No restart unless a new binary actually installed; if the service fails to
  come up after a restart, the log line prints the exact rollback command
- Loopback-only: the service binds 127.0.0.1; remote access is SSH-tunnel-only
- No secrets, no webhooks, no inbound ports
