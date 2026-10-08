# Spec: AI Format (`agent.jsonl`)

Status: draft for #2 · Owner: Product Owner

The AI format is the working substrate: unlimited volume, machine-structured, append-only. Agents write here freely; humans never edit it directly. Records are never rewritten or deleted — corrections are new records that reference what they correct.

## File layout

One JSONL file per unit of work, sitting beside the human format:

```
.tissue/issues/IAM-42/agent.jsonl
.tissue/releases/v1.2.0/agent.jsonl
```

## Record envelope

Every line is one JSON object:

```json
{
  "id": "rec-0192-0007",
  "type": "attempt",
  "ts": "2026-10-08T14:20:00Z",
  "actor": "agent:dev-1",
  "seq": 7,
  "refs": ["rec-0192-0005"],
  "data": { }
}
```

- `id`: unique within the file; monotonic with `seq`.
- `seq`: 1-based, strictly increasing, gap-free per file (cheap integrity check).
- `refs`: optional list of record ids this record builds on or corrects.
- `data`: type-specific payload, defined below.

## Record types

### `plan`
```json
{ "type": "plan", "data": { "goal": "...", "steps": ["...", "..."], "supersedes": "rec-0192-0003" } }
```
Agent's working plan. Supersession is by reference, never edit.

### `observation`
```json
{ "type": "observation", "data": { "summary": "...", "evidence": "commit abc123 / file:line" } }
```
A finding the agent wants to persist (root cause, environment fact, linked-repo state).

### `attempt`
```json
{ "type": "attempt", "data": { "action": "...", "outcome": "success | failure | partial", "detail": "..." } }
```
One tried thing and how it went. Failed attempts are first-class data.

### `result`
```json
{ "type": "result", "data": { "summary": "...", "artifacts": ["path/or/url"] } }
```
Completed work output. Verification evidence belongs here.

### `blocker`
```json
{ "type": "blocker", "data": { "reason": "...", "resolved_by": null } }
```
When null, the blocker is active; a later record sets `resolved_by` to a record id via `refs`.

### `human-sync`
```json
{ "type": "human-sync", "data": { "trigger": "command", "summary_of_human_change": "...", "reconciliation": "steps 3-7 of rec-0192-0003 invalidated" } }
```
Written by the core when the human presses sync. This is the only record type an agent reads *before acting* — it invalidates prior plans.

### `proposal`
```json
{ "type": "proposal", "data": { "proposal_id": "prop-01", "target": "issue.md", "field": "status", "current": "in-progress", "proposed": "closed", "rationale": "...", "state": "proposed | approved | rejected" } }
```
An agent's proposed human-format edit. Requirements:
- Must reference the **exact human-format text** it intends to change (`target` + `field` + `current`). Applying is refused if `current` no longer matches — stale proposals fail safe, they never clobber a newer human edit.
- Creating it sets `tags: [needs-review]` on the human format (via core).
- `state` advances only via `approval` / `rejection` records; the proposal record itself is never edited.

### `approval` / `rejection`
```json
{ "type": "approval", "data": { "proposal_id": "prop-01", "applied": true } }
{ "type": "rejection", "data": { "proposal_id": "prop-01", "reason": "..." } }
```
Human decisions. Approval applies the proposal to the human format and clears `needs-review`. Rejection records the reason — rejections are data the agent learns from; agents must not blindly re-propose the same change (STATE.md, core loop rule 4).

## Invariants (enforced by `tissue-core`)

1. Append-only: records are never modified or reordered; `seq` monotonic and gap-free.
2. Every line parses; a malformed append aborts the whole operation atomically (no partial writes).
3. Proposal lifecycle is a state machine: `proposed → approved | rejected`; any other transition is rejected at write time.
4. Only the core writes `human-sync`, `approval`, `rejection` records; agents write `plan`, `observation`, `attempt`, `result`, `blocker`, `proposal`.
5. Volume is unbounded by design — human-facing surfaces summarize; they never dump this file.
