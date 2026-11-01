# Getting started

From nothing to browsing your issues in the GUI, in three commands.

## Install

**Download a release binary** (recommended):

Grab the archive for your platform from the
[releases page](https://github.com/huukit/tissue/releases):

| Platform | File |
|---|---|
| Linux x86_64 | `tissue-v<version>-x86_64-unknown-linux-musl.tar.gz` |
| Windows x86_64 | `tissue-v<version>-x86_64-pc-windows-msvc.zip` |
| macOS ARM | `tissue-v<version>-aarch64-apple-darwin.tar.gz` |

(replace `<version>` with the release tag you are downloading, e.g. `v0.1.4`)

Use the latest **published** release. Tags are published automatically by the
release workflow, so the newest tag on the releases page is normally the right
pick — but draft releases are hidden from `releases/latest` and from the page
for non-collaborators, so if the newest tag ever appears as a draft, choose the
latest published one instead and match its `.sha256` sidecar.

Verify the download against the published `.sha256` sidecar:

```sh
sha256sum --check tissue-v<version>-x86_64-unknown-linux-musl.tar.gz.sha256
```

Unpack it and put the `tissue` binary (`tissue.exe` on Windows) on your
`PATH`. The Linux build is fully static (musl) — no runtime
dependencies; the others need only their platform's standard runtime.

**Build from source** instead (requires Rust and `git` on `PATH`):

```sh
git clone https://github.com/huukit/tissue
cd tissue/src
cargo build --release
# binary: src/target/release/tissue
```

## The 3-step first run

```sh
tissue init                          # 1. create the issue repo
tissue create DOC-1 "Write docs"     # 2. add your first issue
tissue token issue --label human:me  # 3. issue a write-API token (printed once)
tissue serve                         # 4. browse it
```

Open <http://127.0.0.1:7323> — you should see your issue listed. Paste the
token into the GUI's token bar to unlock write actions; the "+ New issue"
button on the issues list creates issues straight from the browser. The
**Settings** view (sidebar) shows the non-secret config — including the
optional `agent:` block that makes the Sync button invoke your agent — and
the footer shows the version, API version, and license (`GET /api/meta`).

`tissue init` creates a `.tissue/` directory in the current directory
(or the one you name) and initializes it as its own standalone git
repo. Two commits appear: the git `init` and `tissue init: config.yml`.

## Choosing an init mode

| Mode | Command | Use when |
|---|---|---|
| Empty | `tissue init` | Starting from scratch |
| GitHub import | `tissue init --from-github OWNER/REPO` | Migrating existing GitHub issues |
| Filesystem remote | `tissue init --remote /path/to/remote` | Fully offline setup with a local git remote for backup/push |

**GitHub import** requires the `gh` CLI installed and authenticated
(`gh auth login`). Imported issues keep their titles/bodies and get
tissue ids under the `--id-prefix` (default `IAM-1`, `IAM-2`, …).
The import covers **all** issues (open and closed — merged PRs import as
`closed`), maps labels to tags (sanitized to a safe charset; the reserved
`needs-review` tag is never imported), and writes comments to each issue's
`agent.jsonl` as observation records. Re-running is a no-op for existing
ids (`--force` re-imports). For a full dry run of a repo into a scratch
dir, writing nothing but a JSON report:

```sh
tools/import_github_issues.py --repo OWNER/REPO --all \
  --out /tmp/scratch --dry-run --report /tmp/report.json
```

**Filesystem remote** configures a bare git remote at the given path
(created if missing) so every local mutation can be pushed — a
completely offline equivalent of hosting.

## The issue repo is standalone

By product decision ([STATE.md](../operations/product-owner/STATE.md),
decision #2), issue data lives in its **own repository**, never inline
in a code repo. `tissue init` therefore creates the issue repo *next
to* your code checkout, not inside it:

```sh
tissue init --from-github huukit/tissue --dir ~/tissue-issues
```

makes `~/tissue-issues` a self-contained issue repo (own git history,
own config). Use `--dir` whenever you want to control that placement.

## The GUI and its API token

`tissue serve`'s GUI is the full review surface: browse issues (with
project filters and search), work the needs-review queue (approve/reject
in the browser), read the agent-stream timeline, and edit front matter
with conflict-safe semantics.

Write actions in the GUI need the repo's API token. Issue a managed token
(#159) — only its SHA-256 hash is stored in the git-tracked config, so no
usable secret lands in history:

```sh
tissue token issue --label human:you    # prints the secret once — save it
```

Alternatively export `TISSUE_API_TOKEN` in the environment before
`tissue serve` (the env var wins; nothing lands in tracked files; ideal for
the systemd unit via an `EnvironmentFile=`). A legacy `api_token:` line in
`.tissue/config.yml` still works but is discouraged — `tissue token list`
flags it for migration. Revoke with `tissue token revoke <id>`.

In the GUI, paste the token into the token bar once per browser
session; it is kept in the browser's sessionStorage only — never
written to disk, never logged.

Agents use the same token for the REST write API
(`Authorization: Bearer <token>`). See the
[CLI reference](cli-reference.md) for the endpoint list.

## Automated deployment (testing machines)

`deploy/` contains a systemd service + timer that keeps a testing machine
on the latest release automatically: it polls GitHub Releases, sha256-
verifies the download before swapping the binary, refuses to activate a
binary that fails `--version` (rollback copy kept), and never breaks the
loopback-only binding. Both units run as a dedicated unprivileged `tissue`
system user with the binary in `/opt/tissue/bin`. One-time setup (user
creation, token `EnvironmentFile`, troubleshooting) in
[deploy/README.md](../deploy/README.md).

## Where things live

```
myproject-issues/
  .tissue/
    config.yml
    issues/
      IAM-1/
        issue.md      # human format — you edit this
        agent.jsonl   # AI format — agents append to this
    releases/
      v1.2.0/
        release.md
        agent.jsonl
```

## Two machines, one issue repo

The issue repo is a plain git repo, so two machines can share it through any
remote (a bare repo on a NAS, GitHub, an SSH remote). Tissue keeps the history
linear — no merge commits, ever:

```sh
tissue pull    # fetch + rebase; never a merge commit
tissue push    # fast-forward only
```

The discipline is the same as any git-native tool: **pull before you work,
push after you commit**. If both machines edited the same issue in
incompatible ways, `tissue pull` refuses — it aborts the rebase, leaves the
tree untouched, and names the affected issue ids. Resolve by hand with plain
git (`git pull --rebase`, edit the marked text, `git rebase --continue`),
then `tissue pull` again and `tissue push`. The #78 revision guard covers the
record level; this covers the history level.

## Next

- [Workflow](workflow.md) — the dual-format model and the daily loop
- [CLI reference](cli-reference.md) — every command and flag
- [Container guide](container.md) — run the viewer in Docker
- [Automated deployment](../deploy/README.md) — systemd service + auto-upgrade for a testing machine
