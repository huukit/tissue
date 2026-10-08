# Tissue — Roadmap

Project document: where we are and what's next. Milestone definitions live in the tracker; this file tracks direction only.

## Now — Bootstrap

- Product state, problem statement, decisions: `operations/product-owner/STATE.md` ✓
- Repo rules for humans and agents: `AGENTS.md` ✓
- Backlog seeded: issues #1–#7 ✓
- Format specs (#2): `SPEC-human-format.md`, `SPEC-ai-format.md` ✓
- Repo layout: product docs / project docs / sources separated (#4)

## Next — PoC: dogfood switch (#3)

The milestone: switch this project's own issue tracking from GitHub Issues to tissue, run real work through it for two weeks, decide keep/kill on evidence.

- `tissue-core` library (#5)
- `tissue` CLI binary (#6)
- Import issues #1–#3 into the tissue repo (#7)

## After the PoC (only if keep)

- REST API (`tissue serve`) with token auth
- GUI — the sync button gets a home
- Releases implementation (spec exists, PoC skips it)
- Derived indexes / queries
- Multi-repo workflows

## Explicitly not now

Everything in the PoC's out-of-scope list (API, GUI, releases impl, multi-user, remotes/auth, indexes). Scope creep answers here.
