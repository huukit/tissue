# Agent workflow: operating a tissue repo via CLI only

How an AI agent works in a tissue issue repo without touching the human
format directly. This validates the core design bet of #3: **the CLI is
the agent interface**.

## Setup

```sh
tissue token issue --label agent:<your-name>   # run once by a human; the
                                                 # secret is printed once —
                                                 # store it in your env, never
                                                 # in the repo or tickets
# API use:  Authorization: Bearer <token>   (env: TISSUE_API_TOKEN on the serve host)
```

Actor convention: identify yourself as `agent:<name>` on every command
(`--actor agent:dev-1`) and in API `actor` fields. The actor id lands in
the stream record **and** the git commit author — the audit trail depends
on it.

## The four-case contract (#139)

| Action | Who | What happens |
|---|---|---|
| Create an issue | agent | `POST /api/issues` with an `agent:*` actor → **creation proposal**: draft in `agent.jsonl`, `needs-review` in the list, `issue.md` written only on human approval |
| Create an issue | human | `tissue create ID "Title"` (or GUI "+ New issue") → `issue.md` immediately; first `sync` establishes the AI baseline |
| Edit the human format | human | edit `issue.md`, then `tissue sync ID --summary "…"` → field-level diff recorded in a `human-sync` record |
| Edit the human format | agent | `tissue propose …` → proposal + `needs-review`; a human approves/rejects |

**You never write `issue.md`** — not edits, not creation. That is decision
4, and it is enforced structurally (the API path for `agent:*` actors is
proposal-only).

## Daily agent loop

```sh
tissue list                          # triage; [ai-proposal] rows await a human
tissue show TIS-42                   # front matter + body

# read the working stream (timeline in the GUI is the same data):
curl -s http://127.0.0.1:7323/api/issues/TIS-42/records | jq .

# append working records (plan/observation/attempt/result/blocker) via API:
curl -s -X POST http://127.0.0.1:7323/api/issues/TIS-42/records \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"actor":"agent:dev-1","data":{"type":"observation","summary":"root cause: …","evidence":"file:line"}}'

# propose a human-format change (stale-safe: refused if the field moved):
tissue propose TIS-42 --field status --current open --proposed in-progress \
  --rationale "work started" --actor agent:dev-1
# or via API: POST /api/issues/TIS-42/propose {"field","current","proposed","rationale","actor"}
```

The human decides in the GUI queue or CLI (`approve`/`reject`); rejections
carry a reason in the stream — **rejections are data: do not blindly
re-propose the same change** (core loop rule 4).

## Sync and the agent hook (#137)

When a human presses Sync (CLI `tissue sync` or the GUI button), tissue
records the exact field-level diff in a `human-sync` record and — if an
`agent:` block is configured (GUI Settings) — runs that agent command with
a JSON envelope `{issue, revision, changes, stream_tail}` on stdin, cwd =
the issue repo. The outcome (exit code / timeout / duration) is appended as
an `agent-run` record. **Read the latest `human-sync` before acting** — it
invalidates prior plans. If you are the configured agent: append your
records via the API/CLI, exit 0 on success; non-zero exit or timeout is
recorded, never fatal to the sync.

## State you can rely on

- `tissue list` shows derived **last touch** per issue: `[human]` /
  `[synced]` / `[ai-proposal]` (#138, computed from the stream, never stale).
- Pending create-proposals list as `(pending proposal)` rows (#154) and in
  the GUI/API as `needs-review`.
- Every mutation is exactly one git commit (`agent-run` adds a second on
  agent-invoking syncs); `git log` in the issue repo is the audit trail.

## Multi-machine

`tissue pull` before you work, `tissue push` after — rebase-only, no merge
commits (#110). On a conflicting rebase, tissue aborts and names the issue
ids; resolve with plain git, then pull/push again.
