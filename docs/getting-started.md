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

The Linux and macOS tarballs also carry the man page generated from the CLI
definition at `share/man/man1/tissue.1` (#213) — install it and use
`man tissue`:

```sh
sudo install -d /usr/local/share/man/man1
sudo install -m 0644 share/man/man1/tissue.1 /usr/local/share/man/man1/tissue.1
man tissue    # or straight from the unpacked tarball: MANPATH=$PWD/share/man man tissue
```

(Windows has no man format — the zip ships the `docs/` markdown set
instead; `tissue.exe --help` covers the CLI there.)

**Build from source** instead (requires Rust and `git` on `PATH`):

```sh
git clone https://github.com/huukit/tissue
cd tissue/src
cargo build --release
# binary: src/target/release/tissue
```

## Running the test battery in a constrained sandbox (#228)

In a small sandbox (dev session, CI-like container) the process table can
fill up with unreaped zombie children — only a process's parent can reap
it, and a non-reaping PID 1 turns every leaked `git`/test child into a
permanent zombie until `fork()` fails with EAGAIN ("Cannot fork"), which
takes the whole session down. Two layers of protection:

- **Inspect:** `tools/procwatch.py` reports zombies (with the parent pid
  that failed to reap them), per-command counts, and headroom vs `pid_max`;
  it exits non-zero when zombies are present and can clean up live orphaned
  processes. Run it around the battery:

  ```sh
  python3 tools/procwatch.py                 # before; exit 1 = zombies present
  (cd src && cargo test --workspace)
  python3 tools/procwatch.py                 # after — zombies of our own children should not grow
  python3 tools/procwatch.py --kill-orphans  # SIGKILL live orphaned git/tissue leftovers
  ```

- **Guard (automatic):** the integration tests in `src/tissue-cli/tests/`
  spawn every subprocess through `tests/common/mod.rs` — own process group,
  always reaped, group-killed on timeout/drop, a pre-fork `/proc` headroom
  check that fails with a clear message instead of a raw EAGAIN, and a cap
  on simultaneously live children. `tissue-core`'s agent runner kills and
  reaps the agent's process group on every exit path. Root cause of the
  zombie `git` processes specifically: `git commit`/`rebase` spawn a
  detached `git maintenance run --auto` child that git never waits for, and
  only PID 1 can reap the orphan — so all git invocations here run with
  `-c gc.auto=0 -c maintenance.auto=0` (set via `GIT_CONFIG_*` env in the
  test helper so the `tissue` binary's own git children inherit it too).

## The first run

```sh
tissue init                          # 1. create the issue repo (creates the default project)
tissue project add ops OPS           # 2. optional: a second project
tissue create IAM-1 "Write docs" --body "..."   # 3. add your first issue (signed to a project)
tissue token issue --label human:me  # 4. issue a write-API token (printed once)
tissue serve                         # 5. browse it
```

Open <http://127.0.0.1:7323> — you should see your issue listed. Paste the
token into the GUI's token bar to unlock write actions. Projects are the
main unit: the project selector sits at the **top of the left sidebar**,
and the **Projects** page (`#/projects`) lists every project with issue
counts and a "+ New project" button. New issues are always signed to a
project (#172/#176) — the "+ New issue" form has a project selector, there
is no silent default prefix. Issue detail splits **Human** (issue.md) and
**AI** (stream, proposals, approve/reject) into tabs. The
**Settings** view (sidebar) shows the non-secret config — including the
optional `agent:` block (command mode or HTTP-endpoint mode with the API
key kept out of the tracked config) that makes the Sync button invoke your
agent — plus token management (issue/list/revoke; secrets shown once, only
here). The footer shows the version, API version, and license
(`GET /api/meta`). The Releases view has a "+ New release" button.

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

`--from-github` needs the `gh` CLI installed and authenticated (`gh auth
login`, or `GH_TOKEN` in the environment) — without it, init fails fast
with a clear hint before writing anything (#226). Note that `sudo` strips
`gh` from `PATH` (secure_path): use `sudo -E`, the absolute path to `gh`,
or run init as the unprivileged service user (`sudo -u tissue ...`), never
as root — the repo dir is owned by that user (#132).

## The GUI and its API token

`tissue serve`'s GUI is the full review surface: browse issues (with
project filters, status/type dropdowns, and full search over id, title,
body, tags, and status — #212, same matcher as `tissue list --search`),
work the awaiting-decision queue (approve/reject
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
- [Agent workflow](agent-workflow.md) — operating the repo as an agent via CLI only
- [CLI reference](cli-reference.md) — every command and flag
- [Man page](man/tissue.1) — `tissue(1)`, generated from the clap definition (#213)
- [Container guide](container.md) — run the viewer in Docker
- [Automated deployment](../deploy/README.md) — systemd service + auto-upgrade for a testing machine
