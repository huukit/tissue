# Tissue — Roadmap

Project document: where we are and what's next. Milestone definitions live in the tracker; this file tracks direction only.

Principle: **simple before featured.** Every stage must work end-to-end before the next one starts. No stage introduces infrastructure the previous stage didn't prove necessary.

## Done — Bootstrap ✓

- Product state, problem statement, decisions: `operations/product-owner/STATE.md` ✓
- Repo rules for humans and agents: `AGENTS.md` ✓
- Format specs (#2): `SPEC-human-format.md`, `SPEC-ai-format.md` ✓
- Repo layout: product docs / project docs / sources separated (#4) ✓

## Done — PoC core ✓

- `tissue-core` + CLI: full loop (init/create/list/show/sync/propose/approve/reject) (#5, #6)
- Read-only `serve` GUI viewer (#30), releases spec'd (#2), releases impl shipped as git tags (#6, decision 6)
- Release pipeline: tag → 3-platform binaries → draft release (#43–#47, #52)
- User docs (#63), container image (#36/#38)
- v0.1.1 published; owner-verified on remote/tunnel deployment

## Now — Hardening (#65–#74 wave)

Trust before features. The tracker must be boring-correct before anyone depends on it.

- Concurrency correctness: last-writer-wins must die; explicit conflict detection on every human-format mutation (#78)
- Edge-path tests: corrupt files, bad IDs, missing repos/issues (#66/#69, #68)
- Deterministic e2e (no flaky harness) (#72/#73), complete CLI/docs surface (#74)
- Manual test pass on the release binary (#65); dogfood trial alongside GitHub (#67)

## Next — Multi-project (small)

One binary, many projects — without changing the single-project data model.

- Project-level commands inside one issue repo: a project is a named grouping of issue IDs (ID prefixes already carry this naturally); `--project` filter on list/serve views (#79)
- Cross-project queries: `tissue list --all-projects`, aggregated serve view
- Keep the standalone-issue-repo principle (decision 2): multiple projects per repo, never code-repo-inline storage

## Later — REST API (agents' remote surface) (#80)

The API is the agent interface for remote work; the CLI already proved the model.

- Write path via propose/approve only — API never writes the human format directly (decision 4)
- Token auth per issue repo (decision 5, spec'd in STATE.md)
- GUI becomes a real API client; sync button gets its home

## Later — Releases implementation (#81)

Spec exists (decision 6, SPEC-human-format.md); PoC ships tags + release workflow.

- `release create/list/show/assign/cut` commands over `.tissue/releases/<ver>/`
- Issue→release link as front-matter field; per-release derived lists

## Explicitly not now

- Multi-user identity/permissions (actor strings suffice until proven otherwise)
- Remotes/auth infrastructure beyond a plain git remote
- Derived indexes, search
- Webhooks, notifications, integrations
- Anything that adds a server requirement to the CLI-first model

Scope creep answers here.
