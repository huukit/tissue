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

## The two handoffs — the four-case model

Data crosses between the formats only through explicit, trusted
handoffs — never by one side parsing the other's raw edits mid-flight.
Who may do what is fixed (decision 4, creation included):

1. **Agent creates** → proposal: an agent's `POST /api/issues` never
   writes `issue.md`. The draft lives in the agent stream, the issue is
   tagged `needs-review` (shown in `tissue list` as a `(pending proposal)`
   row and in the GUI queue), and `issue.md` is only written when a human
   approves it.
2. **Human creates** → direct: a human's create writes `issue.md`
   immediately (CLI or the GUI's "+ New issue"), and the first **Sync**
   establishes the AI-side baseline.
3. **Human edits** → `issue.md` directly, then **Sync** records the exact
   field-level change (below).
4. **Agent edits** → proposals a human approves or rejects; a rejection
   records the reason as data agents can learn from.

Each issue's derived **last touch** (`human` / `synced` / `ai-proposal`)
is shown in `tissue list`, the API, and as a GUI badge — computed from the
stream on read, never stored, so it cannot drift.

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

If an `agent:` block is configured in `.tissue/config.yml` (Settings page
in the GUI), sync then **runs that agent command** with a JSON envelope
(`{issue, revision, changes, stream_tail}`) on stdin and the issue repo
as its working directory. The agent's own records land through the API/CLI
as usual, and tissue appends an `agent-run` record with the outcome (exit
code, timeout, duration) — a failing or hanging agent is recorded, never
fatal. Without an `agent:` block, sync is record-only.

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
   browser, edit front matter (conflict-safe), sync your edits (which
   also runs the configured agent, see above), and
   create issues and projects right from the list view ("+ New issue",
   "+ New project") — every GUI action lands as the same git commit as
   its CLI twin. The **Settings** view shows and edits the non-secret
   config (`agent:` block, `remote`) via the token-protected config API;
   the API token itself is never displayed there.

**Agent:**
1. Read `issue.md` and `agent.jsonl` for full context.
2. Append working records (notes, attempts, verification) to
   `agent.jsonl`.
3. To change the human format: `tissue propose` and wait for the human
   decision recorded in the stream.

## Every mutation is a git commit

The issue repo is a plain git repo. `init`, `create`, `sync`,
`propose`, `approve`, `reject`, `project`, and `token` each produce
**exactly one commit** (an agent-invoking sync produces two: the sync
record and the `agent-run` outcome) with a readable message:

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
- [CLI reference](cli-reference.md) — every command, flag, and API endpoint
- [CLI reference](cli-reference.md) — every command and flag
- [Agent workflow](agent-workflow.md) — how an agent operates via CLI only (#3 dogfood)

## Dev fixtures for manual testing (#171)

`tissue dev-fixture <ID> --scenario <name> [--seed N]` (hidden from
`--help`) generates an issue whose human format and AI stream are
deliberately out of sync, so you can smoke-test the sync/conflict paths:
`human-edited`, `agent-proposed`, `both-changed`,
`agent-created-pending`, `synced-clean`. It is deterministic (`--seed`)
and every mutation is still a proper git commit. The command **refuses to
run unless `dev_fixtures: true` is set in `.tissue/config.yml`** — enable
that flag only in a throwaway test repo, never in a real one.
