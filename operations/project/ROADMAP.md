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
- `serve` GUI grown through 4 stages into the full review surface: project switcher (#96), releases (#97), awaiting-decision queue + approve/reject + agent timeline (#98), conflict-safe editing + sync (#99), then projects-as-main-unit + Human/AI tabs + create buttons (#178/#170/#134/#135/#173)
- Releases first-class over `.tissue/releases/<ver>/` (#81) + tag-triggered 3-platform binary pipeline (#43–#47, #52)
- REST write API with token auth (#80/#159); GUI is a pure API client (ADR-002); `GET /api/meta` (#143)
- Multi-project (#79/#172/#175/#178), multi-machine pull/push (#110), container image (#36/#38), deploy tooling with auto-upgrade as a dedicated unprivileged user (#83/#131/#132/#167)
- v0.1.1 → v0.1.7 published; owner-verified on the deployment box

## Done — v0.1.5 ✓ / v0.1.6 ✓ / v0.1.7 ✓ (2026-11-01 → 2026-11-05)

- v0.1.5: four-case contract (#139), real sync diff (#136), derived last-touch (#138), agent-invoking sync + settings (#137), GUI create buttons (#134/#135), meta endpoint (#143)
- v0.1.6: token lifecycle hash-only (#159), token visibility settings-only (#174), deploy EROFS guard (#167), HTTP agent mode (#169, decision 13), fixtures (#171), design pass (#168), docs (#147/#190)
- v0.1.7: third-party security review fixes — actor bound to token label (#192), pre-auth body cap (#193), serve concurrency + mutation lock (#194), tag YAML safety (#195), generic API errors (#196) — plus smoke fixes #189/#199/#201/#202/#206/#210 and CI #141. Review archived in `operations/project/reviews/`.

## Done — v0.1.8 ✓ (2026-11-05)

Owner ruling 2026-11-05: v0.1.7 runs; the next release must carry the pending fixes before the extensive smoke pass. Landed on main (all Tester-PASS, PO-approved, FF):

- #200/#205 settings sections + Mistral agent note, #213 man pages, #221 sidebar dropdown, #226 gh preflight — merged earlier
- **#211** issue types + workflow — merged via #225 + the cli.rs fix #231 (the #225 red merge incident → AGENTS.md §7 + #149)
- **#212** search (CLI `--search` + `GET /api/issues?q=` + GUI filters) — PR #229
- **#228** dev-session process-runaway safeguards (procwatch + guarded test harness + gc.auto=0) — PR #230, the sandbox zombie incident fix
- **#233** CI hardening (tools tests, --locked, offline YAML lint, systemd verify) — PR #237
- **#238** repo rules + role skill files (poll mode §8, sandbox hygiene §9) — PO commits `234ee3a`/`232f318`
- Tagged **v0.1.8**. Next: owner extensive smoke (#140 checklist), then the dioxus trial.

## Now — v0.1.9: owner-reported cosmetics after deploying v0.1.8

- **#244** issue detail view unclear — restructure for at-a-glance reading; owner follow-ups from the fixture GUI: label the Human (issue.md) / AI (agent.jsonl) split unmistakably, rename "Edit front matter" → "Edit issue", make the title editable with a Save button
- **#245** Settings garbled — one setting group per section, one setting per line (row grid)
- **#249** bug: draft issue detail (`agent-created-pending`, e.g. IAM-12) returns invalid JSON — missing closing brace in `api_issue_detail` draft branch (serve.rs ~1545) → GUI JSON.parse error
- **#250** format: `summary` as a separate optional front-matter field from `title` (decision 17) — spec, core, CLI, API, GUI list/detail/edit; both editable with Save (#244)
- **#251** GUI: Sync / Approve / Reject must be self-explanatory — visible per-operation explanations on proposal cards, the sync button, and the review-queue header
- **#252** human-side history: `GET /api/issues/<id>/history` + collapsible History section on the Human tab (issue.md's git log)
- **#246/#247** `tools/dev-fixture-demo.sh` one-shot fixture repo (branch `vibe/dev-fixture-demo-246`, box-hardened: root-side mkdir+chown, prod-path hard-refusal)
- **#132/#248** `deploy/install.sh` one-command box setup / migration off root (branch `vibe/deploy-install-132`, box-hardened: binary before init, no curl|grep -m1 EPIPE, /usr/local/bin symlink)
- Gate: Tester PASS → merge → bump 0.1.9 → tag → owner re-review on the box

## Next — owner smoke of v0.1.8 (#140 checklist)

## Next — v0.1.6-dioxus UI trial (#188)

Long-lived branch, never main; UI-only, API frozen (ADR-002); reproduce the current GUI 1:1 in Dioxus; adopt only on owner review, outcome in ADR-003. Runs in parallel with v0.1.8 work; merges only after the trial decision.

## Later — mobile (#239, #240)

- **#239** GUI mobile-responsive layout — the serve UI must be browsable on phones (owner requirement 2026-11-05)
- **#240** mobile app (native or companion over the `/api/*` contract, ADR-002) — after #239 and a stable API

## Later — PoC dogfood switch (#3)

Run tissue's own issues in tissue. Gates: #140 smoke green + #159 ✔ + #149 ✔ (rules verified live) + #192 ✔. Prep done and merged: #162 (import dry-run), #163 (DOGFOOD-RUNBOOK.md + docs/agent-workflow.md).

## Explicitly not now

- Multi-user identity/permissions, OAuth2 — deferred per decision 10 (#85): git-actor identity + per-repo hash-only tokens
- Remotes/auth infrastructure beyond a plain git remote
- Derived indexes, full-text search infrastructure (basic search lands in #212)
- Webhooks, notifications, integrations
- Anything that adds a server requirement to the CLI-first model

Scope creep answers here.
