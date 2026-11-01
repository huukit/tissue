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

## `tissue create <ID> <TITLE>` / `tissue create <ID> --title <TEXT>`

Create a new issue. `ID` must match `[A-Z]+-<number>` (e.g. `IAM-42`).
The title is positional or a `--title` flag — exactly one of the two.

```sh
tissue create DOC-1 "Write user guide"
tissue create DOC-2 --title "Proofread guide" --body "Body text, no editor."
```

Without `--body`, an editor (`$EDITOR`, default `vi`) opens with the
issue template. Writes: `.tissue/issues/<ID>/issue.md`.
Commits: exactly one, `create <ID>: <TITLE>`.

| Flag | Meaning |
|---|---|
| `--title <TEXT>` | The issue title (alternative to the positional form; exactly one required) |
| `--body <TEXT>` | Issue body text; created as-is instead of opening the editor |

---

## `tissue list`

List issues; issues awaiting review are flagged `NEEDS REVIEW`. A pending
agent create-proposal (directory with a stream but no `issue.md` yet,
#139) is listed as `<ID>  (pending proposal)  <draft title>  [ai-proposal] [NEEDS REVIEW]`
— no warning; after approval it becomes a normal row (#154).

```sh
tissue list
tissue list --status open
tissue list --project iam
tissue list --all
```

Read-only.

| Flag | Meaning |
|---|---|
| `--status <STATUS>` | Show only issues with these statuses (`open`, `in-progress`, `closed`) — the full `status` enum; an issue's `status` front-matter field accepts the same three values |
| `--project <NAME>` | Show only issues of one project — a name mapped to an issue-id prefix in `.tissue/config.yml` (see below); unknown names fail with a clear error |
| `--all` | Explicitly list every issue in the repo, across all projects (listing all projects is already the default) |

---

## `tissue pull`

Fetch the remote and rebase onto it — never a merge commit (#110).

```sh
tissue pull
```

On a textual conflict: aborts the rebase (tree untouched), names the affected
issue ids, and tells you to resolve manually with plain git.

---

## `tissue push`

Push to the remote — fast-forward only (#110).

```sh
tissue push                       # plain fast-forward
tissue push --force-with-lease    # after your history was rebased
```

After `tissue pull` rebased your local commits, the plain push is usually
still a fast-forward of the remote; `--force-with-lease` is the correct form
when the remote moved again mid-rebase.

---

## Projects (multi-project repos)

A repo can hold several projects at once: a **project** is a named grouping of
issue-id prefixes (`IAM`, `VER`, …). The mapping lives in `.tissue/config.yml`,
written by `tissue init` (the init prefix becomes the first project) and
extendable by hand:

```yaml
project: e2e
projects:
  e2e: IAM
  second: VER
```

`tissue list --project <name>` and `tissue serve --project <name>` filter by
that prefix; `tissue list --all` lists everything. The default project name
from `init --project <NAME>` doubles as the first key. No schema change: an
issue's project is simply its id prefix.

Manage the mapping with `tissue project` (no hand-editing):

```sh
tissue project list                # name -> prefix, default marked
tissue project add second VER      # add (or overwrite) a mapping
tissue project remove second       # refuse while issues with VER- exist
tissue project remove second --force
```

Every `add`/`remove` is one git commit in the issue repo. `add` refuses a
prefix already mapped to a different name; `remove` refuses while issues with
the project's prefix exist (override with `--force`) and always refuses for
the default `project:` entry. Prefixes are uppercase ASCII letters (the
`[A-Z]+` of issue ids); project names are letters, digits, `-` or `_`.

---

## `tissue show <ID>`

Print one issue: front-matter fields and body. Read-only.

```sh
tissue show IAM-42
```

---

## `tissue sync <ID> --summary <TEXT>`

Record a human edit in the AI stream. Run this **after** editing
`issue.md` — tissue computes the exact field-level changes since the last
sync (title/status/release/tags/body) and records them in the
`human-sync` record, together with a snapshot of the human format at sync
time (the next sync's baseline). Agents reconcile from that data, never
from raw diffs.
Commits: exactly one, `sync <ID>: <summary>`. If nothing changed since the
last sync, the command reports `nothing to sync` and makes no record and
no commit.

If an `agent:` block is configured in `.tissue/config.yml` (see
[Agent connection](#agent-connection) below), sync additionally **invokes
the configured agent**: the command runs with the issue repo as its working
directory, receives a JSON envelope `{issue, revision, changes,
stream_tail}` on stdin, and its outcome (exit code, timeout, duration) is
appended as an `agent-run` record with one more commit. A failing or
hanging agent never fails the sync itself — the failure is recorded as
data. With no `agent:` block, sync stays record-only.

```sh
tissue sync IAM-42 --summary "rewrote acceptance criteria"
tissue sync IAM-42 --summary "..." --actor human:tuomas
```

| Flag | Meaning |
|---|---|
| `--summary <TEXT>` | One-line summary of what you changed and why (required) |
| `--actor <ID>` | Human identity recorded in the stream and commit (default `human:local`) |

---

## Agent connection (`agent:` in `.tissue/config.yml`) (#137)

The `agent:` block configures the command that `tissue sync` and the GUI's
**Sync** button invoke after recording the human edit. The file is
git-tracked and holds **no secrets** — API tokens stay in the environment.

```yaml
project: e2e
agent:
  command: "claude"             # executable resolved on the serve host's PATH
  args: ["-p", "sync {issue}"]  # {issue} is replaced with the issue id
  timeout_sec: 120              # wall-clock timeout; the agent is killed on expiry
  env: {}                       # extra env for the agent process (TISSUE_API_TOKEN is never passed)
```

Missing `agent:` block = sync behaves record-only (backwards compatible).
The spawned process always runs with the issue repo as cwd; the API token
is never in its argv or environment, and nothing is logged from it beyond
a capped stderr tail in the `agent-run` record.

The same settings are viewable and editable in the GUI under **Settings**
via `GET /api/config` (whitelisted keys only — `api_token` is never
returned) and `PUT /api/config` (token-protected; non-whitelisted keys are
rejected, one commit per change).

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
tissue serve --project iam   # filtered viewer
```

| Flag | Meaning |
|---|---|
| `--port <N>` | Port to listen on (default `7323`) |

---

## `tissue token <SUBCOMMAND>` (#159)

Manage the write-API bearer tokens. Secrets are stored as SHA-256 hashes
only in `.tissue/config.yml` (`tokens:` block) — never plaintext.

```sh
tissue token issue --label human:tuomas   # prints the secret once; one commit
tissue token list                          # id, label, created; never the secret
tissue token revoke tok-01                 # revoked tokens fail closed (401)
```

The `TISSUE_API_TOKEN` env var remains as a bootstrap/override and takes
precedence; a legacy plaintext `api_token:` config line still works but is
reported by `token list` as a migration item. See
[API tokens](#api-tokens-159) below for the full resolution order.

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
| `--project <NAME>` | Filter the viewer to one project (name → id prefix from `.tissue/config.yml`); the issues list and detail routes show only that project's issues |

### Config API (#137)

- `GET /api/config` — tokenless read of the **whitelisted** config view: `project`, `remote`, `agent`. `api_token` is never returned.
- `PUT /api/config` — token-protected write: a JSON object of top-level keys (`agent`, `remote`, `project`); `api_token` and any other key are rejected with 400 before anything is written. One `config update via api` commit per change. `{"agent": null}` removes the agent block (sync goes back to record-only).

### Meta API (#143)

`GET /api/meta` — tokenless read of product metadata:

```json
{"product": "tissue", "version": "0.1.4", "api_version": "v1",
 "license": "GPL-3.0-only",
 "license_url": "https://github.com/huukit/tissue/blob/main/LICENSE",
 "build": {"target": "x86_64-unknown-linux-musl", "profile": "release"}}
```

`version` is the workspace version (the release workflow's tag guard keeps
tag == version, #88); `build.target` is the compile-time target triple.
`api_version` is a constant (`v1`) bumped **only on breaking changes** to
the `/api/*` contract — clients should check it before relying on the API.
The GUI footer shows `tissue v<version> · API <api_version> · <license>`
on every view, fetched from this endpoint (with the compiled-in version as
fallback).

### Projects API (#135)

`serve` exposes the project mapping over HTTP — the same tissue-core
module as `tissue project`, so CLI and API cannot drift:

- `GET /api/projects` — tokenless read: `{"default": "e2e", "projects": [{"name": "e2e", "prefix": "IAM", "default": true}, ...]}`
- `POST /api/projects` — token-protected write: `{"name": "ops", "prefix": "OPS", "actor"?: "human:gui"}` → `201 {"created": true}` and one `project add: <name> -> <prefix>` commit; conflicts (duplicate prefix, remapping an existing name) → 409; invalid input → 400.

### API tokens (#159)

The write API authenticates with bearer tokens. Managed tokens are issued,
listed, and revoked with `tissue token`:

```sh
tissue token issue --label human:tuomas    # prints the secret ONCE
tissue token list                           # id, label, created — never the secret
tissue token revoke tok-01                  # fails closed immediately
```

Managed tokens are stored in `.tissue/config.yml` under `tokens:` as
**SHA-256 hashes only** — the plaintext secret is never written to the
git-tracked config, so no usable secret lands in history. Each issue/revoke
is one git commit. The comparison is constant-time (xor-fold over
equal-length inputs).

The token is resolved in this order on each write:

1. **`TISSUE_API_TOKEN` environment variable** (bootstrap/override):
   untracked by git, deployment-friendly — e.g. the systemd unit's
   `Environment=`/`EnvironmentFile=`.
2. **Managed `tokens:` entries** (hash-only) via `tissue token issue`.
3. **Legacy `api_token:` line in `.tissue/config.yml`** (pre-#159 repos):
   still works, but a plaintext token there lands permanently in history —
   `tissue token list` reports it as a migration item. Prefer (1) or (2).

Absent/empty everywhere means the write API is disabled (explicit 503 on
every write attempt); a revoked or wrong token gets 401.
