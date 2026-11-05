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
| 10 | **Identity and auth (#85, owner-signed 2026-11-01): git-actor identity + per-repo static tokens, no OAuth2.** Actor identity is the git actor on each mutation commit — `git log` is the user-management record, no user database. Transport auth for the API/GUI write path is the per-repo static token model (#80): one token for the human, one per agent. OAuth2 is deferred until both a real multi-user team requirement and serve exposure beyond loopback+tunnel exist; if identity *verification* is ever needed, ssh-signed commits are the upgrade path. |
| 11 | **Per-issue last-touch state (#138/#139, owner-ratified 2026-11-01).** Every issue visibly carries who last touched the human format, derived (never stored, never hand-set) from the agent stream + commit actors: `human` (direct human edit/approve/reject), `synced` (a human-sync reconciliation record is the latest event), `ai-proposal` (a proposal awaits review, coincides with the reserved `needs-review` tag). The four interaction cases: AI creates → AI proposes, human approves; human creates → sync establishes the AI-side baseline; human edits → sync reconciles (#136); AI proposes → human decides. Agents never write the human format — creation included (#139 tightens `POST /api/issues` accordingly). |
| 12 | **Sync invokes the configured agent (#137).** `.tissue/config.yml` gains a secret-free `agent:` block (command, args with `{issue}`, timeout, extra env). Sync = record the field-level human-format diff since the last sync baseline + commit, then run the configured agent with a JSON envelope (issue, revision, changes, stream tail); outcome recorded in the stream. No agent block = record-only sync. Tokens/secrets never in argv or logs, never in config. |
| 13 | **Agent integration has two modes (#169, owner-required 2026-11-02): local command (v0.1.5) or HTTP endpoint (OpenAI-compatible URL + model + API key), so tissue can be pointed at a hosted agent (e.g. Mistral) for testing. The endpoint API key is a secret: config.yml stores only a reference (`api_key_ref`); the key itself lives in a gitignored `.tissue/secrets.yml` (auto-added to .gitignore on init) or an env var — never in git history, never returned by GET /api/config, never in argv/logs. The settings page is a normal labeled form (endpoint, model, write-only key field) with the key masked on read.** |
| 14 | **Serve shape (ADR-002, owner-signed 2026-11-05): `tissue serve` = core + REST API + embedded UI in one binary — the default, only required deployment shape (one unit, one auto-upgrading binary, loopback+tunnel). The UI is a pure API client and must remain splittable into a standalone front-end process later without API changes; no second service is ever required for the single-box deploy.** |

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

## Current status (2026-11-02, PO)

- **v0.1.5 is published** (tag `v0.1.5`, main `334c79c`, 6 versioned assets) and deployed on the owner's box; the owner is running the **#140 smoke pass** now.
- Smoke-pass findings so far: #167 (EROFS — box ran the pre-#158 unit; workaround `ReadWritePaths=/srv/tissue-issues`, permanent fix in deploy path), #175 (project not persisted via API/GUI), #176 (issues silently default to IAM- prefix), #168 (GUI controls/design pass), #169 (settings: HTTP agent endpoint + secret key, decision 13), #170 (issue view Human/AI split), #171 (out-of-sync fixture generator), #172 (project assignment on create), #173 (GUI create-release), #174 (token visible only on settings page).
- **main is frozen at v0.1.5 until #140 is green.** Five PRs are verified and waiting behind the freeze: #160 (#159 tokens), #161 (#147 docs), #164 (#162 import dry-run), #165 (#163 runbook), #166 (#150 AGENTS.md note).
- **v0.1.6 scope (next cut):** #167 deploy-unit fix, #175, #176, #169, #174+#159 (token lifecycle + settings tokens), #172, #173, then #170/#168/#171 polish. Owner-side: #149 repo rules, #150 checkbox, #140 smoke pass.
- **#3 dogfood gates:** #140 green + #159 + #149. Prep (#162/#163) already done in PRs #164/#165.
