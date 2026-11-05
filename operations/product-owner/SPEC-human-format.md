# Spec: Human Format (`issue.md` / `release.md`)

Status: draft for #2 · Owner: Product Owner

The human format is the editorial record: small, stable, trusted. It is only ever mutated by direct human edit or approval of an AI proposal (STATE.md, core loop rule 1).

## Projects and config.yml

A repo may hold several projects: a **project** is a named grouping of issue-id
prefixes (`IAM`, `VER`, …). The mapping lives in `.tissue/config.yml`, written by
`tissue init` and extendable by hand:

```yaml
project: e2e
projects:
  e2e: IAM
  second: VER
```

`--project <name>` filters on `tissue list` and `tissue serve` resolve the name
to its prefix here; an issue's project is simply its id prefix. No schema change.

`.tissue/config.yml` also carries two non-secret blocks (#137, #159):

```yaml
agent:                 # optional; absent = sync stays record-only
  command: claude
  args: ["-p", "sync {issue}"]
  timeout_sec: 120
  env: {}
tokens:                # managed API tokens — SHA-256 hashes ONLY, never plaintext
  - id: tok-01
    label: human:tuomas
    sha256: <hex>
    created: 2026-11-01T19:00:00Z
```

The `agent:` block is whitelisted for `GET/PUT /api/config`; `tokens:` and
any `api_token:` line are never returned by the API and never writable
through it (`tissue token issue/list/revoke` manages tokens). Secrets live
in the environment (`TISSUE_API_TOKEN`), never in this git-tracked file.

## File layout

One Markdown file per unit of work, with YAML front matter:

```
.tissue/issues/IAM-42/issue.md
.tissue/releases/v1.2.0/release.md
```

## `issue.md`

```yaml
---
id: IAM-42                  # required, immutable, matches directory name
title: Sync fails on empty agent stream   # required, one line
type: task                 # optional, #211: feature | task | bug (absent = task, backwards compatible)
status: open               # open | in-progress | review | done | rejected (#211; `closed` migrates to `done` on read)
parent: IAM-40             # optional, #211: parent feature id (task/bug only; must exist and be a feature)
release: v1.2.0            # optional; empty/absent = unscheduled (source of truth for issue→release link)
tags: [needs-review]       # free-form list; `needs-review` is reserved: set when a proposal awaits review
created: 2026-10-08T09:00:00Z
updated: 2026-10-08T14:20:00Z
revision: 3                # monotonic; bumped by every human-format mutation. Optimistic concurrency guard (#78): mutating commands refuse to write when the file changed since they read it.
---

## Summary

Human-written summary. A person must understand the issue from this section in under a minute.

## Acceptance criteria

- [ ] First criterion
- [ ] Second criterion

## Notes

Optional human commentary. AI working data does NOT go here — it goes in `agent.jsonl`.
```

### Field rules

- `id`: immutable after creation, matches the directory name exactly. Valid pattern: `[A-Z]+-\d+`.
- `type` (#211, decision 15): `feature` (a collection of tasks/bugs linked via `parent`), `task` (directly implementable), or `bug` (non-conformance report against a feature). Absent = `task`. Only `task`/`bug` may carry a `parent`; a feature cannot.
- `parent` (#211): the parent feature's id. Validated at create/edit/approve time: the parent must exist and be a feature, and the issue must not be its own parent (or a feature).
- `status` (#211): `open | in-progress | review | done | rejected`. Migration: `closed` reads as `done` and is written back as `done` on the next mutation — no data rewrite. `review` is a *human* review state, distinct from the `needs-review` tag (AI awaiting decision, decision 16). Workflow transitions enforced on programmatic changes (API edit, proposal approval): `open→in-progress→review→done`, `review→in-progress` (changes requested), `any→rejected`, `rejected→open` (reopen). `done` stamps the release that shipped it when a release link exists.
- `release`: the single canonical place an issue→release link is edited; per-release lists are derived from scanning this field.
- `tags.needs-review`: set by the core automatically when a proposal is created; cleared on approval or rejection. Must never be hand-added silently — presence always means "an unreviewed proposal exists."
- `last_touch` (#138, derived-only — never stored in front matter): computed on read from the agent stream: `ai-proposal` while a proposal is pending; `synced` when the last stream record is a `human-sync` reconciliation; `human` otherwise (direct edits, approvals, or no stream). Shown by the CLI list, the API summaries/detail, and the GUI badge. Being derived, it cannot drift from the stream.
- `updated`: bumped by the core on every human-format mutation; `created` is set once.
- `type`/`parent` (#211): part of the human format — edited by humans directly or via approved proposals like any other field; the sync diff records them.

### Invariants (enforced by `tissue-core`)

1. Parseable front matter + body; unknown front-matter keys are preserved, not dropped (forward compatibility).
2. The body is human-authored text only; the core never appends AI working data to it.
3. A `done` issue with no `release` requires explicit human confirmation (the pre-#211 `closed` state migrated to `done` on read).

## `release.md`

```yaml
---
id: v1.2.0                 # matches directory name; semver for PoC
title: First cut
status: draft              # draft | cut
target: 2026-11-01         # optional date
created: 2026-10-08T09:00:00Z
updated: 2026-10-08T14:20:00Z
---

## Goals

What this release is for.

## Notes

Release notes draft. AI may propose note edits; approval flow applies as for issues.
```

- `status: cut` is the finalized state; cutting a release = git tag `v<id>` + close-out commit (STATE.md, decision 6).
- Linked issues are derived from `issue.md` front matter; `release.md` never duplicates the list.
