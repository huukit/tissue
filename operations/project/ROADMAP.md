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

## Done — v0.1.4 (2026-11-01) ✓

Trust before features held: the v0.1.4 gate landed all of it FF on main (`5f550a3`→`efe4ec8`).

- Versioned artifact names + upgrade script resolves by asset list (#115/#116); releases publish directly, `draft: false` (#126); published-vs-draft rule documented (#127)
- **#118** fixed: sync records carry `reconciliation`; strict open accepts legacy records (#123)
- Docs refresh for the real GUI surface (#114/#117); stale "read-only viewer" tagline fixed (#133)
- `tissue project add/list/remove` (#119), serve no-token startup warning (#120)
- Deploy hardening from the owner's box: literal `ExecStart` (203/EXEC, #129), file-based release fetch (curl exit 23, #130), post-swap `--version` guard (#128), manual-install docs for versioned names

## Now — v0.1.5: make the core loop real and usable from the GUI

Owner-verified on the deployment machine; next cut is about the sync button and visible state.

- **#139** four-case contract: agents never write the human format (creation included) — agent `POST /api/issues` becomes propose+needs-review; human actors create directly
- **#136** sync records the real field-level human-format diff since the last baseline; "nothing to sync" when clean; #118 regression test
- **#138** derived per-issue `last_touch` state (`human` / `synced` / `ai-proposal`) in CLI list, API, GUI badges
- **#137** settings page + `agent:` block in config.yml; sync invokes the configured agent, outcome recorded in stream
- **#134** GUI "New issue" button (human actor) · **#135** GUI "New project" button + `POST/GET /api/projects` sharing validation with CLI #119
- Deploy: **#131** startup troubleshooting docs, **#132** units as dedicated `tissue` user, **#128** close-out
- Gate: dev PRs → Tester verify → PO merge FF → tag v0.1.5 → owner smoke pass (auto-upgrade chain, full GUI write loop incl. sync diff, conflict path, rollback, loopback check) → then #3 discussion

## Later — PoC dogfood switch (#3)

Manage tissue's own issues in tissue. Explicitly gated by the owner: not before the v0.1.5 smoke pass is green on the deployment machine. Prerequisites (pull/push, import tooling, GUI review surface) are in place.

## Explicitly not now

- Multi-user identity/permissions, OAuth2 — deferred per decision 10 (#85): git-actor identity + per-repo static tokens until a real team requirement exists
- Remotes/auth infrastructure beyond a plain git remote
- Derived indexes, search
- Webhooks, notifications, integrations
- Anything that adds a server requirement to the CLI-first model

Scope creep answers here.
