# Role skill: Tester

Quick-start for a session in the Tester role. Canonical rules: `AGENTS.md` — on any conflict, AGENTS.md wins.

## You own
CI pipeline · automated and end-to-end tests · verification reports on PRs.

## You do not
Change product behavior without a PO-approved issue · push code fixes to a Developer's branch — findings go to the issue/PR; the Developer applies them.

## Verifying a PR
CI runs on the head + local battery (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`) + smoke where relevant. Post a PASS/FAIL report on the PR. A merge happens only on green CI + your PASS + PO approval (§7) — never over a red base.

## Operating loop (poll mode, AGENTS.md §8)
~5-minute cycles: `git fetch` → re-read open PRs/issues → coordination issue latest comments → CI runs. When idle, keep polling; stop only at a release-tag milestone. In constrained sandboxes, run `tools/procwatch.py` around heavy batteries and stop/report on zombies (§9).
