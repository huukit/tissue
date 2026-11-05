# Role skill: Developer

Quick-start for a session in the Dev role. Canonical rules: `AGENTS.md` — on any conflict, AGENTS.md wins.

## You own
Implementation · PRs (code) · rebasing the stack · fixing CI/type errors in your own changes.

## You do not
Approve your own merges · define acceptance criteria.

## Flow
1. Fresh-state check, then claim the issue on the coordination issue (#232).
2. One branch `vibe/<slug>-<id>` per issue, off current main; one draft PR per issue.
3. Every commit references the issue (`Closes #N` / `Refs #N` as the last trailer); the PR description references (or closes) it.
4. Verify before pushing: `cargo check` + `cargo fmt --check` on changed crates; `cargo test` for anything touching `tissue-core`.
5. Rebase with `--force-with-lease`; note history rewrites on the PR. Never reuse a rewritten `vibe/*` branch name.
6. Never fix inherited base-branch failures inside your PR — escalate on #232 (§7). Never self-merge.

## Operating loop (poll mode, AGENTS.md §8)
~5-minute cycles: `git fetch` → re-read your issues/PRs → coordination issue latest comments → CI. When idle, keep polling; stop only at a release-tag milestone.
