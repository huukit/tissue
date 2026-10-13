# Tissue — Product State

Living record of product-owner decisions. This directory is the PO playground: specs, decisions, and strategy live here. Nothing here is implemented code — it is the source of truth for what tissue *is* and why.

## Problem statement

The way we track software development work was designed for a world where humans did the work and humans read the record. That world is over.

AI agents now perform a large and growing share of development work. They explore, attempt, fail, retry, and document at a volume orders of magnitude beyond what a human will ever read. In conventional trackers, one of two things happens: the AI's output is hidden or discarded — hard-won context evaporates with the session — or it is dumped into the ticket, and the human-facing record becomes unusable noise. Teams lose either **the agent's memory** or **the human's ability to understand their own project**.

Humans also face a trust problem: when AI can freely read and write the shared record, humans can no longer tell what they decided versus what a machine inferred, and the source of truth stops being trustworthy.

The failure is structural: current tools give humans and AI a single shared representation of work when the two need different things from it. Humans need a small, stable, editorial record. AI needs unlimited, structured working memory.

**Tissue fixes this structurally, not cosmetically.** Every unit of work carries two connected formats — a human format humans own, and an AI format agents append to — reconciled only through explicit, trusted handoffs.

**Solved when:** a developer understands any issue in under a minute regardless of agent work on it; an agent picks up any issue knowing everything worth knowing; and no change to the human record ever happens without human approval.

## Vision

Tissue is a place where humans and AI build things together, without drowning each other in their own output. Every piece of work has a human face and an AI mind, kept separate but connected.

### Differentiators
1. Trust model first — AI proposes, humans decide, everything auditable.
2. Zero infrastructure — issues are files in a git repo you own; data outlives the product.
3. The issue is a living record, not a helpdesk ticket.

## Locked decisions

| # | Decision |
|---|----------|
| 1 | **Git as backbone.** No database. Files are the source of truth; indexes are derived and gitignored. |
| 2 | **One standalone issue repo per project.** Issue data lives in its own repo, never inline in the code repo. Code repos are referenced, not assumed. |
| 3 | **Dual format per unit of work.** `issue.md` (human format, YAML front matter + body) and `agent.jsonl` (AI format, append-only). |
| 4 | **Editing contract.** Human→AI: explicit **sync** (the sync button / `tissue sync`) — the agent reconciles the AI stream on demand, never from raw diffs. AI→human: **propose → tag → approve** — agents never write the human format directly; proposals are tagged for review, humans approve (applies the edit) or reject (recorded as data, not error). |
| 5 | **Three interfaces over one core.** CLI (humans + agents, ships first), REST API (agents/remote), GUI (review, sync button lives here). All are thin layers over a single core library; no interface implements business logic. |
| 6 | **Releases are first-class.** Releases live in `.tissue/releases/<ver>/` with the same dual format. Issue→release link is a front-matter field on the issue (source of truth); per-release lists are derived. Cutting a release = git tag + close-out commit. Agents can propose release-note drafts and readiness reports. |
| 7 | **Implementation language: Rust.** Core, CLI, and API in one Rust workspace; GUI in TypeScript as a pure API client. |
| 8 | **License: GPL-3.0-only.** Copyleft (#51). The full license text is `LICENSE` at the repo root; the Cargo workspace declares `GPL-3.0-only`. |
| 9 | **CI on GitHub-hosted runners.** Standard `ubuntu-latest`/`windows-latest`/`macos-latest` labels — free and unlimited for public repos. The self-hosted runner interlude (#47) was reverted once this was confirmed (#52). |

## Implementation direction

- **Core + CLI + API:** Rust
  - `clap` — CLI
  - `gix` or git CLI shell-out — git operations
  - `serde` + `serde_yaml` / `serde_json` — formats
  - `axum` — `tissue serve` API with token auth (v1: single token per issue repo)
  - OpenAPI spec (utoipa or hand-written) for agent tooling
- **Crate layout:** `tissue-core` (formats, sync loop, proposal state machine, git) and `tissue-cli` (binary incl. `serve` subcommand). Core stays UI-free.
- **GUI:** TypeScript web frontend consuming the API.
- **Risks to manage:** keep the core simple (plain enums/functions for the state machine); keep the API surface tiny.

## Draft CLI surface (MVP)

```
tissue init
tissue create
tissue list [--needs-review]
tissue show <id>
tissue sync <id>
tissue propose <id>
tissue approve <id>
tissue release create <ver>
tissue release list
tissue release show <ver>
tissue assign <id> <ver>
tissue release cut <ver>
```

## Storage shape (draft)

```
<project>-issues/
  .tissue/
    config.yml
    issues/
      IAM-1/
        issue.md
        agent.jsonl
    releases/
      v1.2.0/
        release.md
        agent.jsonl
```

## Core loop rules

1. The human format is only ever mutated by direct human edit or approval of an AI proposal.
2. Human-format changes never propagate to the AI format without explicit sync.
3. Every proposal is tagged and visible in lists; work continues, review is asynchronous.
4. Rejections are data, not errors — the agent stream learns from them.
5. Sync and approval are recorded as git commits; the whole loop is auditable in repo history.

## Open questions

1. **Wedge:** individual developers (bottom-up) vs. teams (replace Jira)?
2. **Scope of work:** software issues only, or any human+AI collaborative work?
3. **Openness:** product, protocol, or both? Should the file formats become a standard?

## Next steps

- Spec `issue.md` front matter schema and `agent.jsonl` record types (incl. release records).
- Spec API resource list.
- Build order: core library → CLI → API → GUI.
