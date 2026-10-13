# Spec: Human Format (`issue.md` / `release.md`)

Status: draft for #2 · Owner: Product Owner

The human format is the editorial record: small, stable, trusted. It is only ever mutated by direct human edit or approval of an AI proposal (STATE.md, core loop rule 1).

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
status: open               # open | in-progress | closed
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
- `status`: the three values above. `closed` stamps the release that shipped it when a release link exists.
- `release`: the single canonical place an issue→release link is edited; per-release lists are derived from scanning this field.
- `tags.needs-review`: set by the core automatically when a proposal is created; cleared on approval or rejection. Must never be hand-added silently — presence always means "an unreviewed proposal exists."
- `updated`: bumped by the core on every human-format mutation; `created` is set once.

### Invariants (enforced by `tissue-core`)

1. Parseable front matter + body; unknown front-matter keys are preserved, not dropped (forward compatibility).
2. The body is human-authored text only; the core never appends AI working data to it.
3. A `closed` issue with no `release` requires explicit human confirmation.

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
