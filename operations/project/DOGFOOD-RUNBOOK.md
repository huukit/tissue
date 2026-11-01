# Dogfood cutover runbook (#3)

Status: draft for PO review (#163). Work on a branch; **merge only after the gates: #140 green + #159 (PR #160) + #149**.

This runbook is the cutover-day procedure for switching tissue's own issue
tracking from GitHub Issues to a tissue issue repo, plus rollback and the
two-week keep/kill evaluation mapped to #3's acceptance criteria.

## Preconditions (all required before cutover day)

- [ ] #140 owner smoke pass green on the deployment box (v0.1.5 auto-upgrade chain verified)
- [ ] #159 merged (token lifecycle; hash-only storage) and #149 repo rules active (owner-only PR approval, `v*` tag pushes restricted)
- [ ] #162 merged (import hardening) — its dry-run report for huukit/tissue is clean: 163/163 issues, 0 failures, 642 comments, labels mapped, none dropped
- [ ] A scratch full import (not dry-run) reviewed: spot-check 10 issues incl. the 314-comment coordination issue #1
- [ ] `tissue serve` + GUI write loop verified locally on the imported scratch repo (token → new issue → edit → sync → propose → approve)

## Cutover day

1. **Freeze GitHub Issues.** Post an archive note on GitHub: "Issues move to
   the tissue repo on <date>; this tracker is archive-only from now. PRs and
   CI stay on GitHub." Do not close/lock the repo (PR references #NN must
   keep resolving).
2. **Create the real issue repo** next to the code checkout:

   ```sh
   tissue init --from-github huukit/tissue --dir ~/tissue-issues --id-prefix TIS
   # imports ALL issues (open+closed; merged PRs as closed), comments to agent.jsonl
   ```

   Idempotent: re-running skips existing ids (`--force` re-imports).
3. **Remote setup** — filesystem remote first (fully offline-capable), optional
   GitHub private remote for off-box backup:

   ```sh
   cd ~/tissue-issues
   tissue init --remote /srv/tissue-remote   # only if re-initializing; otherwise:
   git remote add origin /srv/tissue-remote && git push -u origin main
   ```

   (For the initial import, pass `--remote` to `init` in step 2 instead.)
4. **Point agents at the tissue repo.** Agents work there via CLI/API only —
   see [AGENT-WORKFLOW.md](AGENT-WORKFLOW.md). Issue the write token out of
   band (never in chat/tickets):

   ```sh
   tissue token issue --label human:<you>    # GUI token; printed once
   tissue token issue --label agent:<name>   # per-agent token (decision 10)
   ```
5. **Sanity pass:** `tissue list` shows all imported issues with correct
   statuses/tags; `tissue serve` on 127.0.0.1:7323 shows the needs-review
   queue empty (import never sets `needs-review`); `git log` is legible.
6. **Announce** in the repo's own first tissue-native issue (create it with
   the CLI/GUI): cutover done, links to this runbook.

## Daily loops after cutover

- **Human:** `tissue list` → triage; GUI needs-review queue for
  approve/reject; edit `issue.md` → `tissue sync <ID> --summary …` (records
  the field-level diff and runs the configured agent hook, #137).
- **Agent:** CLI only — read, append records, propose; never writes
  `issue.md` (four-case contract, #139). See AGENT-WORKFLOW.md.
- **Multi-machine:** `tissue pull` before work, `tissue push` after —
  rebase-only, never merge commits (#110).

## What happens to PR/CI references

GitHub PRs stay on GitHub (code review is not issue tracking). Issue bodies
keep their `#NN` references; they resolve to GitHub PRs/issues as before.
New tissue issues reference PRs the same way (plain `#NN` in the body).

## Rollback (to GitHub Issues)

1. Post "back on GitHub Issues" note; unfreeze.
2. `git -C ~/tissue-issues remote remove origin` if it pointed at a private GitHub remote — or simply stop using the tissue repo; the filesystem remote keeps the full history either way.
3. New issues go to GitHub again; the tissue repo stays as an archive (it is a plain git repo — readable forever).
4. Revoke agent tokens: `tissue token revoke <id>` (fails closed, #159).

## Two-week keep/kill evaluation (mapped to #3 ACs)

Run this review after 14 days of real use; decide keep/kill with evidence:

| #3 AC | Evidence source | Keep criterion |
|---|---|---|
| AC1 all new work in tissue repo | `tissue list` vs GitHub Issues (should be empty of new work) | >90% of new issues created in tissue |
| AC2 end-to-end loop on real work | git log: syncs, proposals, approvals with actors | every agent change went through propose→approve; zero direct agent writes to issue.md |
| AC3 legible git log | `git log --oneline` of the issue repo | a human can reconstruct what happened and who did it from commit subjects alone |
| AC4 clean `tissue show` under heavy agent activity | pick the 3 most agent-active issues | front matter + body readable; stream volume lives in agent.jsonl/timeline |
| AC5 evidence-based decision | this table filled with numbers + a short write-up posted as a tissue issue (meta!) | keep if AC1–AC4 hold; kill/rollback per above if the loop adds friction humans route around |

## Open choices (PO calls at cutover)

- Open-only vs full archive import — #162's dry run imported all 163 (open+closed+merged); recommendation on record: full archive, so history stays complete in one repo.
- Hosting: filesystem remote (offline-first) vs GitHub private remote as origin.
