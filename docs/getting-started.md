# Getting started

From nothing to browsing your issues in the GUI, in three commands.

## Install

**Download a release binary** (recommended):

Grab the archive for your platform from the
[releases page](https://github.com/huukit/tissue/releases):

| Platform | File |
|---|---|
| Linux x86_64 | `tissue-x86_64-unknown-linux-musl.tar.gz` |
| Windows x86_64 | `tissue-x86_64-pc-windows-msvc.zip` |
| macOS ARM | `tissue-aarch64-apple-darwin.tar.gz` |

Verify the download against the published `.sha256` sidecar:

```sh
sha256sum --check tissue-x86_64-unknown-linux-musl.tar.gz.sha256
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
tissue init                      # 1. create the issue repo
tissue create DOC-1 "Write docs"  # 2. add your first issue
tissue serve                     # 3. browse it
```

Open <http://127.0.0.1:7323> — you should see your issue listed.

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

## Next

- [Workflow](workflow.md) — the dual-format model and the daily loop
- [CLI reference](cli-reference.md) — every command and flag
- [Container guide](container.md) — run the viewer in Docker
