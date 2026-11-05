# Source

Rust workspace for tissue. Layout:

- `tissue-core/` — the core library: dual-format model (`issue.md` + `agent.jsonl`), git-commit-per-mutation, sync reconciliation, proposals, releases, projects. No I/O policy; all interfaces are thin layers over this.
- `tissue-cli/` — the `tissue` binary: CLI (humans + agents) and `tissue serve` (REST API + embedded GUI).
- `gui/` — the embedded GUI: TypeScript source (`app.ts`) compiled to `app.js` (checked in; CI verifies it matches a fresh strict `tsc` compile). Pure API client — see `operations/project/adr/ADR-002`. A Dioxus rewrite is being trialed on a branch (#188).
- `Cargo.toml` — workspace root; `version` here is the release version (the tag guard enforces tag == version, #88).

## Build & test

```sh
cargo build --release          # from this directory
cargo test                     # full suite incl. deterministic e2e
cargo fmt --check && cargo clippy --workspace --all-targets
npx -y -p typescript@5.6 tsc --strict --noEmitOnError --target es2017 gui/app.ts --outFile /tmp/app-fresh.js && cmp gui/app.js /tmp/app-fresh.js
```

CI runs all of the above on every PR. User docs live in `../docs/`; product state in `../operations/`.
