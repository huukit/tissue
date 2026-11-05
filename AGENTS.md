# Repository / Agent Rules

These rules apply to all contributions — human and AI agent alike.

## 0. Roles

Three roles operate on this repository. A role is a responsibility, not an identity: any agent or human may hold any role on a given task, but exactly one role owns each step of the workflow.

A PR may contain work from multiple roles, provided each commit's message attributes its role (e.g. `Dev: / Test:` prefix or a clear description). Role boundaries apply to approval and verification — the Product Owner approves, the Tester verifies — not to the authorship of a PR as a whole. Splitting a PR by role is not required. CI/type-error fixing applies to findings in the Developer's own changes; failures inherited from the base branch are escalated to the owning role via the coordination issue, not silently fixed in whoever's PR is on top.

| Role | Owns | Does not |
|------|------|----------|
| Product Owner | Backlog and issue creation/prioritization · approving merges · final say on scope and rules · `operations/product-owner/` (specs, vision, decisions) | Writing code or tests |
| Developer | Implementation · PRs (code) · rebasing the stack · fixing CI/type errors in their own changes reported by the Tester via CI results and review findings | Approving own merges · defining acceptance criteria |
| Tester | CI pipeline · automated and end-to-end tests · verification reports on PRs | Changing product behavior without a PO-approved issue |

### Working agreements

- Communication happens through issues (labeled coordination), never through files in branches. Cross-agent conflicts are resolved in the coordination issue before either side rebases.
- Issue ownership: the Product Owner creates issues; Developer and Tester may propose issues but the PO prioritizes them.
- Review: the Tester verifies PRs (CI, smoke tests, review reports); the Product Owner approves merges. No agent merges its own PR.
- Boundaries: the Tester does not push code fixes to a Developer branch — findings go to the issue/PR; the Developer applies them.
- Fresh state before responding: an agent's memory is not a source of truth. Before any substantive response — status reports, coordination answers, review replies, planning — re-sync with the live repository: `git fetch`, then re-read the current PRs/issues and their latest comments. Answer from what was just read, and cite what was checked (branch SHAs, PR numbers, comment timestamps). If an earlier statement conflicts with fresh state, correct it explicitly rather than silently dropping it.
- Role quick-starts: `operations/roles/product-owner.md`, `operations/roles/developer.md`, `operations/roles/tester.md`. Every agent reads its role file at session start; this file (AGENTS.md) remains canonical on any conflict.

## 1. Every change references an issue

- Work must be planned in a tracked GitHub issue before code is written.
- Every commit must reference its issue in the message body:
  - `Closes #N` when the commit completes the issue,
  - `Refs #N` when the commit is a step toward it.
- Every pull request must reference (or close) its issue in the description, so the tracker links the work.

Do not open PRs or push commits that reference nothing. Do not invent issue numbers; if no issue covers the work, create one first.

## 2. Linear history only — no merge commits, ever

- The default branch (`main`) advances only by fast-forward or rebase.
- Merge commits are forbidden.
- Keep PR branches rebased on top of their target so they can be merged clean (FF). If the target moves, rebase and force-push the PR branch (`--force-with-lease`).
- Make small, focused commits; no fixup noise on the shared history.
- If commits are rewritten after review (e.g. fixing a message), rebase dependent stacked branches on the rewritten commits, push with `--force-with-lease`, and note the rewrite on the PR.

## 3. Commit attribution

- Author and `Co-authored-by` trailers must reflect who actually wrote the code. Never add a `Co-authored-by` trailer for someone who only reviewed or requested the change.
- Commit bodies keep their issue reference (`Closes #N` / `Refs #N`) as the last trailer.

## 4. Development workflow

- One branch + one PR per issue, named `vibe/<short-slug>-<id>`.
- Verify before pushing: at minimum `cargo check` and `cargo fmt --check` on changed crates; `cargo test` for anything touching `tissue-core`.
- Draft PRs are opened for review; they are never self-merged.
- Scope discipline: stay within the issue's scope; unrelated changes belong in their own issue/PR.
- Branch lifecycle (#150): the repo setting "Automatically delete head branches" is on, so a merged PR's branch is deleted by GitHub right after the merge — the remote should only ever contain `main` plus the branches of currently open PRs. Never reuse a `vibe/*` branch name after a force-push rewrite (stale local/remote refs pointing at the old head caused the #110/#115 claim races); if a branch was rewritten, push with `--force-with-lease` and expect the old name to be gone. When cleaning up by hand, only delete a remote branch after verifying it is merged (`git merge-base --is-ancestor <branch> main`).

## 5. Security baseline

- No credentials or secrets in tracked files. Configuration examples use placeholder values only (e.g. `.env.example`, `config.example.yml`).
- API auth tokens are read from the environment or a gitignored config file, never hardcoded or committed.
- Validate all untrusted input at the boundary (API request bodies, front-matter parsing of issue files); errors are handled explicitly, never `unwrap`ped on external data.
- Filesystem operations stay inside the issue repo working tree; resolve and reject path traversal in any user-supplied issue or release identifiers.

## 6. Communication style

- PR descriptions and comments on PRs: concise and accurate. Lead with the answer or decision, no filler, no restating known context. Always end with a short summary a reviewer can act on — PRs must stay reviewable from their comments alone.
- Comments on coordination-labeled issues: verbosity allowed. Agents exchanging information there benefit from completeness.
- Commit messages: existing rules (§1, §3) suffice; no additional style requirements.

## 7. Merge safety — never over a red base

- Before any merge: the PR's merge-ref CI run must be green — that is the exact tree that lands, so a red head is never mergeable (#225 merged red on 2026-11-05, leaving main uncompilable and bricking every open PR's CI). Main's latest push CI run should also be green; if it is red, a merge is allowed only when the PR's green merge-ref run covers that failure (merged tree compiles and passes — e.g. the PR that fixes a broken base) and the base failure is escalated on the coordination issue. Never merge to get past an unexplained red base.
- Never merge with any failed required check, even under time pressure. Branch protection enforcing this is tracked in #149.

## 8. Agent runtime — poll mode

- Every agent session runs in ~5-minute poll loops. Each cycle: `git fetch`, re-read open PRs/issues and the coordination issue's latest comments, check the latest CI runs — then act on what changed, citing what was checked (see the fresh-state rule in §0).
- When idle, keep polling; do not stop or end the session at idle. The only allowed stop is reaching a release-tag milestone (the vX.Y.Z tag of the current milestone) — announce it on the coordination issue.

## 9. Sandbox hygiene (#228)

- In a constrained sandbox, run `python3 tools/procwatch.py` (lands with #230) before and after heavy local batteries (`cargo test --workspace` and friends). If it exits 1 (zombies present), stop spawning processes and report on the coordination issue — never keep testing into a full process table (`fork()` EAGAIN kills the session).
- Test batteries must go through the guarded harness from #230 (own process group, always reaped, group-killed on timeout); do not bypass it.
