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
- `serve` GUI viewer (#30) grown into the full review surface in 4 stages: shell + project switcher (#96), releases view (#97), needs-review queue + approve/reject + agent timeline (#98), human editing + sync with revision-guard conflicts (#99)
- Releases first-class: `release create/list/show/assign/cut` over `.tissue/releases/<ver>/` (#81), tag-triggered 3-platform binary pipeline (#43–#47, #52)
- REST API on `serve`: agent write path (append/propose) with token auth (#80); GUI is a real API client
- Hardening: revision guard on human-format mutations (#78), edge-path tests (#66/#69/#68), deterministic e2e (#72/#73), docs surface (#74), manual pass on release binaries (#65)
- Multi-project: prefix-based grouping, `--project` filters on list/serve, GUI project switcher (#79)
- Multi-machine issue repos: `tissue pull` (rebase-only) / `tissue push` (FF-only, force-with-lease after rebase) (#110)
- Canonical agent-stream record shape (#106); user docs (#63); container image (#36/#38); deploy tooling with auto-upgrade (#83)
- v0.1.1 → v0.1.3 published; owner-verified on the remote/tunnel deployment

## Now — v0.1.4 gate and release-quality fixes

Trust before features. The tracker must be boring-correct before anyone depends on it.

- Release gate: versioned artifact names (#115/#116) + docs refresh (#114/#117) — Tester-verified, then tag v0.1.4
- **#118**: API record-append 400s on any synced issue (writer/reader schema disagreement, #106 class) — found in the v0.1.3 manual pass; fix before the next release
- Owner verification on the deployment machine (follow-up to #65): the system must be fully working and owner-verified there **before** any dogfood switch

## Next — deployment-UX gaps (small, from the owner's v0.1.3 test)

- `tissue project add/list/remove` — manage the multi-project config without hand-editing `config.yml` (#119)
- `serve` warns at startup when no API token is configured, instead of silent 401s on writes (#120)

## Later — PoC dogfood switch (#3)

Manage tissue's own issues in tissue. Explicitly gated by the owner: not before the system is verified working on the deployment machine. Prerequisites (pull/push, import tooling, GUI review surface) are in place.

## Explicitly not now

- Multi-user identity/permissions, OAuth2 — deferred per decision 10 (#85): git-actor identity + per-repo static tokens until a real team requirement exists
- Remotes/auth infrastructure beyond a plain git remote
- Derived indexes, search
- Webhooks, notifications, integrations
- Anything that adds a server requirement to the CLI-first model

Scope creep answers here.
