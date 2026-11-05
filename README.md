# Tissue

<img src="operations/product-owner/logo.svg" width="180" alt="Tissue logo" align="right" />

**AI makes the mess. Humans keep it clean.**

Tissue is an issue tracker for software development *with AI in the loop* — built on the premise that humans and AI need different things from the record of work.

## Why

AI agents now do a large share of development work, and they produce context at a volume no human will ever read: explorations, failed attempts, retries, verification runs. Conventional trackers force that output into a single shared ticket, so you lose one of two things you can't afford to lose — the agent's memory, or the human's ability to understand their own project.

Tissue's answer is structural: **every unit of work carries two connected formats.**

- **Human format** (`issue.md`) — small, stable, editorial. Owned and edited by humans.
- **AI format** (`agent.jsonl`) — unlimited, structured, append-only. Owned by agents.

Neither side overwrites the other without an explicit, trusted handoff:

- **Human → AI:** press *sync*. The agent reconciles its working stream with your edit — never from raw diffs mid-task.
- **AI → human:** agents *propose* changes to the human format; the issue is tagged for review; a human approves or rejects. Rejections are recorded as data, so agents learn from them.

## Git is the backbone

No database, no server required. Tissue stores everything as files in a plain git repo — one standalone issue repo per project:

```
myproject-issues/
  .tissue/
    config.yml
    issues/
      IAM-1/
        issue.md        # human format
        agent.jsonl     # AI format, append-only
    releases/
      v1.2.0/
        release.md
        agent.jsonl
```

Your issues are files you own, versioned in git: every sync, proposal, and approval is an auditable commit. Releasing is native too — cutting a release is a git tag, and issues link to releases as a simple front-matter field.

## Interfaces

Three skins over one core, so humans and agents always see the same truth:

| Interface | For |
|---|---|
| **CLI** | Humans and agents, local — ships first |
| **REST API** | Remote/agent access, token-authenticated |
| **GUI** | Review and triage — where the sync button lives |

## Repository layout

| Path | Content |
|---|---|
| `operations/product-owner/` | Product documents: vision, state, branding, format specs |
| `operations/project/` | Project documents: roadmap, decision records |
| `src/` | Source code — Rust workspace (`tissue-core`, `tissue-cli`) |

## Status

Working PoC, feature-complete through the GUI roadmap. Shipped: `tissue-core` + `tissue-cli` (init/create/list/show/sync/propose/approve/reject/release/project/token, every mutation a git commit, optimistic revision guard on concurrent edits), multi-machine `pull`/`push` with rebase-only semantics, and the full `tissue serve` GUI — projects as the main unit (selector on top of the sidebar + a Projects page with counts and "+ New project"), issue list with "+ New issue" and per-project filtering, issue detail with Human/AI tabs, needs-review queue with browser approve/reject, releases view with "+ New release", conflict-safe front-matter editing, a Settings page (agent config incl. HTTP-endpoint mode, token management), and a version/license footer — over a token-authenticated REST write API. Sync records the exact field-level human-format diff and, when an `agent:` block is configured in `.tissue/config.yml`, invokes that agent command (timeout-bounded, token never exposed to it) and records the outcome. Write tokens are managed with hash-only storage (`tissue token issue/list/revoke`); `GET /api/meta` exposes version/license/API-version. Plus a container image path, automated testing-machine deployment running as a dedicated unprivileged user with unit-staleness guards (`deploy/`), and a tag-triggered release workflow. Licensed GPL-3.0-only. The product state lives in [`operations/product-owner/STATE.md`](operations/product-owner/STATE.md), the roadmap in [`operations/project/ROADMAP.md`](operations/project/ROADMAP.md). Stack: Rust core/CLI, TypeScript GUI. Next milestone: the [PoC dogfood switch](https://github.com/huukit/tissue/issues/3) — run tissue's own issues in tissue; gated on the #140 smoke pass, #159 (landed), and #149 repo rules (owner action).

## Getting started — standalone

Two commands and a browser. The release binary is fully static (musl); no Rust
toolchain, no node, no server dependencies.

1. **Get the binary** — download the build for your platform from the
   [releases page](https://github.com/huukit/tissue/releases) — always the
   latest **published** release (tags are published automatically; skip any
   tag still marked draft): static
   Linux x86_64 (`*-unknown-linux-musl`), Windows x86_64
   (`*-pc-windows-msvc`), or macOS aarch64 (`*-aarch64-apple-darwin`).
   Each artifact has a `.sha256` sidecar. Or build from source:
   `cd src && cargo build --release`
2. **Bootstrap a project** — `tissue init --from-github huukit/tissue`
   creates `tissue-issues/` with this repo's own issues imported (via `gh`;
   run `gh auth login` first), or `tissue init --remote /path/to/remote` for a
   fully offline filesystem remote, or bare `tissue init` for an empty repo
3. **View it** — `cd <your-project>-issues && tissue serve`, then open
   <http://127.0.0.1:7323>

The viewer binds `127.0.0.1` exclusively — nothing is exposed beyond your
machine. Reads need no token; write actions (approve/reject, edit, sync)
require the repo's API token — issue one with `tissue token issue` (only
its hash is stored) — entered once per browser session (kept in
sessionStorage only, never on disk).

Full walkthrough — install variants, all three init modes, sha256
verification, and the standalone-issue-repo principle — lives in
[docs/getting-started.md](docs/getting-started.md). The daily
human/agent loop (sync, propose, approve/reject) is documented in
[docs/workflow.md](docs/workflow.md), and every command and flag in
[docs/cli-reference.md](docs/cli-reference.md).

## Getting started — container

The PoC runs as a single static binary in a minimal container: GUI
assets are embedded in the binary, so the final image contains
nothing else. Build the image, mount an issue repo, and browse it —
the viewer is loopback-only by design, so use `--network host` on
Linux:

```sh
docker build -f Dockerfile -t tissue:poc .
docker run --rm --network host \
  -v /path/to/myproject-issues:/issues tissue:poc \
  serve --port 8080 --repo /issues
```

Read-only CLI commands work through the entrypoint too. Build steps,
the `--network host` rationale, and what stays on the host: see
[docs/container.md](docs/container.md).

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).

## The joke

The name works on every level: a *tissue* wipes up the mess, and *issue* is right there inside it.

*wipe responsibly.*
