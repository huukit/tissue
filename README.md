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

Early stage — product spec and decisions are settled, implementation is just beginning. The product state lives in [`operations/product-owner/STATE.md`](operations/product-owner/STATE.md), the roadmap in [`operations/project/ROADMAP.md`](operations/project/ROADMAP.md). Planned stack: Rust core/CLI/API, TypeScript GUI. Current milestone: the [PoC dogfood switch](https://github.com/huukit/tissue/issues/3) — run tissue's own issues in tissue.

## Getting started — standalone

Two commands and a browser. The release binary is fully static (musl); no Rust
toolchain, no node, no server dependencies.

1. **Get the binary** — download `tissue` from the
   [releases page](https://github.com/huukit/tissue/releases) (static Linux
   x86_64), or build from source: `cd src && cargo build --release`
2. **Bootstrap a project** — `tissue init --from-github huukit/tissue`
   creates `tissue-issues/` with this repo's own issues imported (via `gh`;
   run `gh auth login` first), or `tissue init --remote /path/to/remote` for a
   fully offline filesystem remote, or bare `tissue init` for an empty repo
3. **View it** — `cd <your-project>-issues && tissue serve`, then open
   <http://127.0.0.1:7323>

The viewer is read-only and loopback-only: it binds `127.0.0.1` exclusively
and refuses every non-GET request — nothing is exposed beyond your machine.

## The joke

The name works on every level: a *tissue* wipes up the mess, and *issue* is right there inside it.

*wipe responsibly.*
