# ADR-001: Rust core, git backbone, dual-format storage

- Status: accepted
- Date: 2026-10-08
- Owners: Product Owner (with repo owner)

## Context

Tissue needs a core for an issue tracker whose premise is a human format and an AI format per unit of work, reconciled by explicit sync and propose/approve handoffs. Storage must have no database; the repo is one standalone issue repo per project.

## Decision

1. **Rust** for core, CLI, and API. Correctness of the trust model is the product; encode the sync and proposal state machines so illegal states are unrepresentable. Single static binary; `clap`, `gix` (or git CLI shell-out), `serde`, `axum` later for the API.
2. **Git as the backbone.** Files are the source of truth; indexes derived and gitignored; every mutation is a commit; release cut = git tag.
3. **Dual format** per unit of work: `issue.md` (YAML front matter + Markdown, human-owned) and `agent.jsonl` (append-only records, agent-owned). Specs: `operations/product-owner/SPEC-human-format.md`, `SPEC-ai-format.md`.
4. **TypeScript GUI** as a pure client of the API, decoupled from the Rust decision.

## Consequences

- Slower MVP iteration than Go/TS; mitigated by keeping the core small and boring (plain enums, no clever async).
- No rich queries until derived indexes exist; PoC scans files.
- Data outlives the product; no lock-in; offline-first for free.
- Stale proposals fail safe: applying requires the current human-format text to still match, so agents never clobber human edits.

## Alternatives considered

- Go core: faster MVP, weaker type-level encoding of the trust model.
- Node/TS core everywhere: better agent-ecosystem fit, weaker distribution story for the CLI.
- Database-backed storage: rejected — destroys the zero-infrastructure and audit-trail properties.
