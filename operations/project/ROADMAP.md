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

## Done — v0.1.5 (2026-11-01, published 19:01Z) ✓

main @ `862f3c8`, linear, CI green on head. The core loop is real and usable from the GUI:

- **#139** four-case contract: agents never write the human format (creation included) — agent `POST /api/issues` is a creation proposal + needs-review; human actors create directly; sync-on-empty establishes the AI-side baseline
- **#136** sync records the real field-level human-format diff since the last baseline; "nothing to sync" when clean; #118 regression covered
- **#138** derived per-issue `last_touch` (`human` / `synced` / `ai-proposal`) in CLI list, API, GUI badges
- **#137** settings page + `agent:` block in config.yml; sync invokes the configured agent (process-group timeout kill), outcome recorded in the stream
- **#134** GUI "New issue" · **#135** GUI "New project" + `POST/GET /api/projects` (core module shared with CLI #119) · **#143** `GET /api/meta` + version/license/API-version footer · **#145** `tissue create --title` · **#154** CLI list/show mark pending create-proposals
- Deploy: **#131/#132** units run as the dedicated `tissue` user (`/opt/tissue/bin`, scoped `ReadWritePaths`, EnvironmentFile token, ExecStartPre repo guard) + "first start fails?" troubleshooting docs; #128 close-out
- Release pipeline now self-sufficient: publish directly (#126), versioned assets (#115), auto-upgrade verified end-to-end on the owner's box

**main is frozen at v0.1.5 until the owner's #140 smoke pass is green (testing happens 2026-11-02).** Open PRs waiting behind the freeze: #160 (#159 token lifecycle), #161 (#147 docs).

## Now — v0.1.6 release (2026-11-05, in progress)

main @ `6194f50` (ADR-002/decision 14). All smoke-pass findings + trust/auth merged except the last two PRs (#185 token visibility, #187 design pass — rebased, awaiting Tester re-verify). Then: PO bumps to 0.1.6, tags, verifies published release per the #148 runbook, owner re-smoke (#140 follow-up on the box).

Merged this wave: #159 token lifecycle · #167 deploy EROFS guard · #169 settings/HTTP agent (decision 13) · #170 Human/AI tabs · #171 fixtures · #172+#176 project-on-create · #173 new-release API+GUI · #174/#175/#178/#147/#150/#162/#163.

Owner ruling 2026-11-01/02: no dogfood switch before proper authentication (#159 ✔ landed, #149 owner action pending).

## Next — v0.1.6-dioxus: UI trial on a branch (#188)

Owner-planned, starts only after v0.1.6 is tagged and stable. Long-lived branch, never main; **UI changes only, no features, API frozen** (ADR-002). Reproduce the current GUI 1:1 in Dioxus as a pure API client; adopt only if the owner's review says it beats the current GUI, else discard and continue. Decision recorded in ADR-003.

## Then — v0.1.7: bugs & findings

Whatever the trial decides: remaining bugs/findings, #149 verification, #141 CI confirm, and the #3 dogfood prep follow-ups.

## Later — PoC dogfood switch (#3)

Manage tissue's own issues in tissue. Gates: #140 green + #159 + #149. Prep done: #162 (import dry-run, merged), #163 (runbook + agent workflow doc, merged).

## Later — PoC dogfood switch (#3)

Manage tissue's own issues in tissue. Gates: #140 green + #159 + #149. Prep may proceed on branches now (merge after the gates):

- **#162** import hardening + full dry-run import of huukit/tissue into a scratch repo
- **#163** [DOGFOOD-RUNBOOK.md](DOGFOOD-RUNBOOK.md) (cutover day procedure, rollback, two-week keep/kill criteria) + [docs/agent-workflow.md](../../docs/agent-workflow.md) (agent CLI-only workflow: actor conventions, four-case contract, token use, sync/agent hook)

## Explicitly not now

## Explicitly not now

- Multi-user identity/permissions, OAuth2 — deferred per decision 10 (#85): git-actor identity + per-repo static tokens until a real team requirement exists
- Remotes/auth infrastructure beyond a plain git remote
- Derived indexes, search
- Webhooks, notifications, integrations
- Anything that adds a server requirement to the CLI-first model

Scope creep answers here.
