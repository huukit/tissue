# Workflow: two formats, two handoffs, one git history

## The dual-format model

Every unit of work (issue, release) carries **two connected files**:

```
.tissue/issues/IAM-42/
  issue.md      human format — small, stable, editorial. Humans own it.
  agent.jsonl  AI format — unlimited, structured, append-only. Agents own it.
```

Humans need a short, trustworthy record; agents need unlimited working
memory (explorations, failed attempts, retries, verification runs).
Forcing both into one file loses one or the other. Tissue separates
them structurally — **neither side writes the other's file directly.**

- `issue.md` is YAML front matter (`id`, `title`, `status`, `release`,
  `tags`, `created`, `updated`) plus a Markdown body you write. The
`status` field accepts exactly three values: `open`, `in-progress`,
`closed`.
- `agent.jsonl` is one JSON record per line, strictly append-only,
  with a contiguous sequence number per issue.

## The two handoffs

Data crosses between the formats only through explicit, trusted
handoffs — never by one side parsing the other's raw edits mid-flight.
Who may do what is fixed (decision 4, creation included):

- **An agent never writes `issue.md`** — not edits, not creation. An
  agent's `POST /api/issues` is a **creation proposal**: the draft lives
  in the agent stream, the issue is tagged `needs-review`, and `issue.md`
  is only written when a human approves it.
- **A human creates or edits `issue.md` directly** (CLI or GUI), then
  presses **Sync** — on a fresh issue the first sync establishes the
  AI-side baseline; after an edit it records the exact field-level change.
- **An agent's edits are proposals** a human approves or rejects; a
  rejection records the reason as data agents can learn from.

**Human → AI: `tissue sync`.**
After you edit `issue.md`, run

```sh
tissue sync IAM-42 --summary "rewrote acceptance criteria"
```

This appends a `human-sync` record to `agent.jsonl` and commits it. The
record carries the **exact field-level changes** since the last sync
(title/status/release/tags/body — computed by tissue, not retyped by you),
plus a snapshot of the human format at sync time. Agents reconcile their
working stream from that data onward — they never have to diff your file
to discover your intent. If nothing changed since the last sync, tissue
says `nothing to sync` and makes no record and no commit.

**AI → human: propose → tag → approve/reject.**
An agent never edits `issue.md`. It proposes:

```sh
tissue propose IAM-42 --field status --current open \
  --proposed in-progress --rationale "work started" --actor agent:dev-1
```

The proposal is recorded in `agent.jsonl`, the issue is tagged
`needs-review` (surfaced as `NEEDS REVIEW` in `tissue list`), and a
human decides:

```sh
tissue approve IAM-42 prop-01            # applies the edit, clears the tag
tissue reject  IAM-42 prop-01 --reason "not started yet"
```

Rejections are recorded as data, not errors — the reason is written to
the stream so agents can learn from it. Proposals are fail-safe: if the
field's value changed since the proposal was made, the apply is
refused as stale.

## The daily loop

**Human:**
1. `tissue list` — triage; anything tagged `NEEDS REVIEW` needs you.
2. Read the proposal (`tissue show IAM-42`), then `approve` or `reject`.
3. Edit `issue.md` as needed, then `tissue sync IAM-42 --summary ...`.
4. `tissue serve` for the GUI: triage the needs-review queue, read
   proposals with before/after context, approve or reject them in the
   browser, edit front matter (conflict-safe), sync your edits, and
   create projects ("+ New project") — every GUI action lands as the
   same git commit as its CLI twin.

**Agent:**
1. Read `issue.md` and `agent.jsonl` for full context.
2. Append working records (notes, attempts, verification) to
   `agent.jsonl`.
3. To change the human format: `tissue propose` and wait for the human
   decision recorded in the stream.

## Every mutation is a git commit

The issue repo is a plain git repo. `init`, `create`, `sync`,
`propose`, `approve`, and `reject` each produce **exactly one commit**
with a readable message:

```
create IAM-1: First issue
sync IAM-1: rewrote acceptance criteria
propose IAM-1 (prop-01): status open -> in-progress
```

`git log` is therefore a legible audit trail of everything anyone —
human or agent — did, and who did it (`--actor` is recorded in both the
stream and the commit).

## Where to next

- [Getting started](getting-started.md) — install and first run
- [CLI reference](cli-reference.md) — every command and flag
