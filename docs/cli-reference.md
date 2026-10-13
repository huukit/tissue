# CLI reference

Every command supports the global flag:

| Flag | Meaning |
|---|---|
| `--repo <DIR>` | Path to the tissue repository (the directory containing `.tissue/`). Defaults to the current directory. |

---

## `tissue init`

Initialize a tissue repository: create the `.tissue/` structure,
optionally configure a remote and/or import GitHub issues.

```sh
tissue init                                      # empty repo in current dir
tissue init --from-github huukit/tissue          # import a GitHub repo's issues
tissue init --remote /path/to/remote             # offline filesystem remote
tissue init --from-github huukit/tissue --dir ~/tissue-issues
```

Writes: a new git repo containing `.tissue/config.yml` (and
`.tissue/issues/…` when importing). Commits: `tissue init`,
`tissue init: config.yml`, plus one `create …` commit per imported issue.

| Flag | Meaning |
|---|---|
| `--project <NAME>` | Project name written to `.tissue/config.yml` (defaults to the directory name) |
| `--from-github <OWNER/NAME>` | Import existing issues from a GitHub repository as part of setup (needs `gh` authenticated) |
| `--remote <PATH>` | Configure a filesystem git remote at this path (fully offline); created if it does not exist |
| `--dir <DIR>` | Issue repository location (default: `<project>-issues/` under the current directory when a remote or import source is given, otherwise the current directory) |
| `--id-prefix <PREFIX>` | Tissue id prefix for imported issues (default: `IAM`) |
| `--dry-run` | Show what init would do, write nothing |

---

## `revision` (front matter)

Every human-format mutation bumps a monotonic `revision` field in `issue.md`.
Mutating commands (`propose`, `approve`, `reject`) capture the file content at
read time and refuse to write — exiting non-zero with
`conflict: issue changed while you were working — re-read it and retry` — if
the file changed on disk in between (e.g. another window or machine edited it).
Nothing is auto-resolved or clobbered: re-read the issue and re-run the command.
Older files without the field parse as revision 1.

## `tissue create <ID> <TITLE>`

Create a new issue. `ID` must match `[A-Z]+-<number>` (e.g. `IAM-42`).

```sh
tissue create DOC-1 "Write user guide"
tissue create DOC-2 "Proofread guide" --body "Body text, no editor."
```

Without `--body`, an editor (`$EDITOR`, default `vi`) opens with the
issue template. Writes: `.tissue/issues/<ID>/issue.md`.
Commits: exactly one, `create <ID>: <TITLE>`.

| Flag | Meaning |
|---|---|
| `--body <TEXT>` | Issue body text; created as-is instead of opening the editor |

---

## `tissue list`

List issues; issues awaiting review are flagged `NEEDS REVIEW`.

```sh
tissue list
tissue list --status open
```

Read-only.

| Flag | Meaning |
|---|---|
| `--status <STATUS>` | Show only issues with these statuses (`open`, `in-progress`, `closed`) — the full `status` enum; an issue's `status` front-matter field accepts the same three values |

---

## `tissue show <ID>`

Print one issue: front-matter fields and body. Read-only.

```sh
tissue show IAM-42
```

---

## `tissue sync <ID> --summary <TEXT>`

Record a human edit in the AI stream. Run this **after** editing
`issue.md` — agents reconcile from this record onward, never from raw
diffs. Appends a `human-sync` record to `agent.jsonl`.
Commits: exactly one, `sync <ID>: <summary>`.

```sh
tissue sync IAM-42 --summary "rewrote acceptance criteria"
tissue sync IAM-42 --summary "..." --actor human:tuomas
```

| Flag | Meaning |
|---|---|
| `--summary <TEXT>` | One-line summary of what you changed and why (required) |
| `--actor <ID>` | Human identity recorded in the stream and commit (default `human:local`) |

---

## `tissue propose <ID> --field <F> --current <V> --proposed <V>`

Propose a change to an issue's human format for review. Fails safe if
the field no longer has the `--current` value (stale proposals are
refused). Tags the issue `needs-review` and appends the proposal to
`agent.jsonl`. Commits: exactly one.

```sh
tissue propose IAM-42 --field status --current open \
  --proposed in-progress --rationale "work started" --actor agent:dev-1
```

| Flag | Meaning |
|---|---|
| `--field <FIELD>` | Field to change: `title`, `status`, or `release`. `status` accepts exactly: `open`, `in-progress`, `closed` |
| `--current <VALUE>` | The value the field currently has (stale proposals are refused) |
| `--proposed <VALUE>` | The value the field should have |
| `--rationale <TEXT>` | Why the change is right, recorded for the human reviewer |
| `--actor <ID>` | Agent identity recorded in the stream and commit (default `agent:local`) |

---

## `tissue approve <ID> <PROPOSAL_ID>`

Approve a pending proposal: applies the edit to `issue.md`, appends the
decision record, and clears `needs-review`. Commits: exactly one.

```sh
tissue approve IAM-42 prop-01
```

| Flag | Meaning |
|---|---|
| `--actor <ID>` | Human identity recorded in the stream and commit (default `human:local`) |

---

## `tissue reject <ID> <PROPOSAL_ID> --reason <TEXT>`

Reject a pending proposal: records the reason so agents can learn from
it, and clears `needs-review`. Rejections are data, not errors.
Commits: exactly one.

```sh
tissue reject IAM-42 prop-01 --reason "premature: work not started"
```

| Flag | Meaning |
|---|---|
| `--reason <TEXT>` | Why it is rejected (required) |
| `--actor <ID>` | Human identity recorded in the stream and commit (default `human:local`) |

---

## `tissue serve`

Serve the read-only local viewer (GUI) on `127.0.0.1`. **Loopback-only:
it binds `127.0.0.1` exclusively, refuses every non-GET request, and
cannot be exposed to the network.** Stop it with Ctrl-C.

```sh
tissue serve                 # http://127.0.0.1:7323
tissue serve --port 8080
```

| Flag | Meaning |
|---|---|
| `--port <N>` | Port to listen on (default `7323`) |

---

## `tissue release <SUBCOMMAND>`

Release tracking over `.tissue/releases/<ver>/` (STATE.md decision 6):
each release carries the dual format like an issue — `release.md` (human)
plus an append-only `agent.jsonl` stream. The issue→release link lives
in the issue's `release` front-matter field; per-release lists are
derived from it, never stored twice.

### `tissue release create <ID> <TITLE>`

Create a new release (draft). `ID` is `v` + semver (e.g. `v1.2.0`).
Writes: `.tissue/releases/<ID>/release.md`. Commits: exactly one,
`release create <ID>: <TITLE>`.

```sh
tissue release create v1.2.0 "Second cut" --body "Goals and notes."
```

| Flag | Meaning |
|---|---|
| `--body <TEXT>` | Release notes / goals text (default: empty) |
| `--actor <ID>` | Human identity recorded in the commit (default `human:local`) |

### `tissue release list`

List every release: id, status (`draft` / `cut`), title.

### `tissue release show <ID>`

Print one release's front matter and body, plus its assigned issues —
derived from each issue's `release` front-matter field.

### `tissue release assign <ISSUE_ID> <RELEASE_ID>`

Assign an issue to a release: writes the `release` front-matter link on
the issue and appends an `assignment` record to the release's stream.
Commits: exactly one. Reassigning an issue that already has a release
fails — clear the link first (an empty `--proposed ""` on the field via
`propose`/`approve`).

### `tissue release cut <ID>`

Finalize a release: sets `status: cut` (close-out commit), appends a
`release-cut` stream record, and creates the git tag — the release id
*is* the tag name (`v1.2.0` id → `v1.2.0` tag; ids are `v`-prefixed by
validation). Cutting an
already-cut release fails. Commits: exactly one, `release cut <ID>`.
