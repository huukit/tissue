# Tissue — Full Code Review & Status Assessment (2026-11-05)

Reviewed commit: `1df51b1` (main). Scope: `src/tissue-core`, `src/tissue-cli`
(incl. `serve`/API), `src/gui`, CI/CD, deploy, and product/process docs.
Static review only (no local Rust toolchain in this environment); CI on
GitHub (`gh run list`) confirms the head commit builds, tests, lints, and
`cargo audit`s green, so findings below are logic/architecture issues, not
compile failures.

## TL;DR status assessment

Tissue is a genuinely well-run PoC for its stage: a disciplined process
(`AGENTS.md`, role separation, linear history, issue-driven commits), a
mature CI pipeline (fmt, clippy `-D warnings`, tests, `cargo audit`, MSRV
pin, static-binary + container-content verification, anti-merge-commit
guard), careful secret handling (hash-only tokens, constant-time compare,
scrubbed config reads, sessionStorage-only GUI token, escape-first Markdown
renderer), and solid unit/integration test coverage (~3,000 lines of
`tissue-cli/tests/*` across e2e, edge cases, pull/push, releases, serve,
settings, GUI review flows).

The one structural gap is significant enough to call out before anything
else: **the write API's actor identity is entirely self-asserted by the
request body and is never bound to the token that authenticated the
request.** This undermines the product's core trust model (STATE.md
differentiator #1, decision 11, #139's "four-case contract") — see Finding
1. Everything else below is secondary: a pre-auth DoS in the hand-rolled
HTTP server (Finding 2), a concurrency/availability gap from the
single-threaded request loop (Finding 3), and smaller robustness/design
notes.

---

## Findings

### 1. 🔴 HIGH — Actor identity is self-asserted, not bound to the auth token (trust-model bypass)

**Where:** `src/tissue-cli/src/serve.rs` (`api_write_route`,
`api_write_create_issue`, `api_write_propose`, `api_write_decide`,
`api_write_sync`, `api_write_edit`, `api_put_route`) and
`src/tissue-cli/src/api_write.rs` (`actor_from`, `create_issue`).

**What happens:** Every write endpoint checks only *whether* the bearer
token is valid:

```rust
match bearer_token(headers) {
    Some(provided) if resolve_token(repo, &provided).is_some() => {}
    ...
}
```

`resolve_token` actually returns the token's **label** (e.g. `"human"` or
`"agent:dev-1"`, set at `tissue token issue --label …`), but every handler
discards it and instead trusts an `"actor"` field read straight out of the
**unauthenticated JSON request body**:

```rust
let actor_id = json_field(&v, "actor").unwrap_or("agent:api");
...
match crate::api_write::create_issue(repo, id, title, body, actor_id, project) { ... }
```

`create_issue`'s entire human/agent branch — the mechanism implementing
decision 11/#139 ("agents never write the human format; their create is a
needs-review proposal") — is just a string check:

```rust
if actor_id.starts_with("agent:") {
    // proposal path (needs-review)
} else {
    // direct write — issue.md written immediately
}
```

Since `actor_id` is attacker/caller-controlled and never checked against
the token that authenticated the request, **any holder of any valid write
token — including one issued and labeled `agent:*` — can set
`"actor": "human:owner"` in the request body and bypass the proposal/review
workflow entirely**:

- `POST /api/issues` with `actor: "human:x"` creates the issue directly
  instead of filing a needs-review creation proposal.
- `POST /api/issues/<id>/edit` with `actor: "human:x"` writes front-matter
  fields directly (bypasses the "agents never write the human format"
  rule, decision 4).
- **Most damaging:** `sync.rs::Core::decide` (approve/reject) performs
  *no* actor-type check at all (confirmed by reading the function body —
  it only checks the proposal's state machine). An agent can propose a
  change with its own token and then immediately call
  `POST /api/issues/<id>/approve` with `actor: "human:x"` to **self-approve
  its own proposal**, defeating the single mechanism ("AI proposes, humans
  decide") the whole product is built around.

This is untested: `tissue-cli/tests/api_write.rs` and
`tests/settings_agent.rs` issue both a `human` and an `agent:dev-1` token,
but no test asserts that an agent-labeled token is prevented from acting as
`human:*`, or that `decide`/`edit` require a human-bound actor.

**Fix:** Bind the actor used for workflow-gating to the *resolved token's
label*, not to a client-supplied field. Concretely:
- Have `resolve_token` (or the route layer) return the actual `TokenEntry`,
  and derive `actor_id` from `entry.label` server-side; drop (or at most
  use as a display-only sub-identity, e.g. `human:<token-label>:<free text
  suffix>`) the client's `"actor"` field for anything that affects
  workflow routing.
- Add an explicit actor-kind check in `sync::Core::decide` (approve/reject
  must come from a token whose resolved label is `human` or `human:*`)
  rather than relying on callers to self-report.
- Add regression tests: agent token + `actor: "human:*"` must still be
  treated as an agent for creation/edit, and `decide` must reject a
  decision presented by anything other than a human-labeled token.

### 2. 🟠 HIGH — Pre-auth unbounded allocation from `Content-Length` (DoS)

**Where:** `src/tissue-cli/src/serve.rs::handle` (~lines 95–120).

```rust
let mut content_length: u64 = 0;
...
if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:")
    .and_then(|v| v.trim().parse().ok())
{
    content_length = v;
}
...
let mut body = Vec::new();
if content_length > 0 {
    body = vec![0u8; content_length as usize];   // <-- no upper bound
    reader.read_exact(&mut body) ...
}
```

A client can send any `Content-Length` value (e.g. tens of GB) before any
route dispatch or auth check runs, causing an immediate large allocation
attempt. This runs **before** `write_auth_configured`/token checks, so it
needs no valid token — just a TCP connection to the port. On the stated
deployment shape ("loopback+tunnel", decision 14/ADR-002, with multiple
remote agents expected per decision 10), this port is routinely exposed to
more than the literal local machine. Confirmed: no `cfg.audit`/size check
exists in CI or code for this path.

**Fix:** Cap request-body size (e.g. a few MB; everything this API accepts
is JSON issue/record/config payloads) and reject with `413` before
allocating, and/or stream into a fixed-size buffer and 400 on overflow
rather than trusting the header value verbatim.

### 3. 🟡 MEDIUM — Single-threaded request loop (availability/concurrency)

**Where:** `src/tissue-cli/src/serve.rs::serve`

```rust
for stream in listener.incoming() {
    match stream {
        Ok(s) => { if let Err(e) = handle(s, &repo, &gui_root, project_prefix) { ... } }
        Err(e) => eprintln!("warning: connection failed: {e}"),
    }
}
```

Requests are handled strictly sequentially on the accept loop, with no
thread-per-connection or async executor. Combined with Finding 2, one slow
or malicious connection stalls every other client. It also means a
`sync` call that invokes a slow/hanging configured agent (`timeout_sec`,
default 120s — `config.rs::run_agent`/`run_agent_http`) blocks the *entire*
viewer/API for up to that long for every other concurrent user or agent,
even though the agent subprocess itself is correctly isolated in its own
process group with a bounded timeout. This is a reasonable PoC starting
point but should be called out explicitly as a scaling limit before the
product's stated direction (more agents, more concurrent write tokens)
proceeds further.

**Fix:** At minimum, spawn a thread per connection (the codebase already
uses blocking I/O throughout, so `std::thread::spawn` per accept is a small
change); longer-term, consider a request queue with a worker pool sized to
avoid concurrent git mutations on the same repo (two concurrent writers
racing `commit_all` is also a correctness question worth testing once this
changes — `human::IssueFile::write_if_unchanged`'s optimistic-concurrency
guard is read-then-write with no lock, which is safe today only because
nothing is actually concurrent).

### 4. ⚪ LOW — Tag serialization has no escaping (latent, currently unreachable)

**Where:** `src/tissue-core/src/human.rs::IssueFile::serialize`

```rust
(
    "tags",
    if self.tags.is_empty() { "[]".to_string() }
    else { format!("[{}]", self.tags.join(", ")) },
),
```

Tags are emitted as a raw, comma-joined YAML flow sequence with no
per-element escaping. If any tag value ever contained `,`, `]`, `:`, or a
YAML special character, this would corrupt the front matter on the next
write (and likely break `IssueFile::parse` on a subsequent read — a
self-inflicted data-corruption bug). Today this is **not exploitable**:
the only call site that adds a tag is `add_tag(NEEDS_REVIEW, …)`, a fixed
constant (`"needs-review"`), and there's no public API or CLI surface to
add arbitrary tags. Flagging because `tags` is listed as a diffable field
in `sync.rs`'s field-diff (`push("tags", before.tags.join(" "), ...)`) and
as the format gets more surface (free-form tagging is an obvious future
feature), this will become a real parsing bug/YAML-injection vector the
moment tags become human- or agent-settable text.

**Fix:** Either restrict tag values to a safe charset at the point of
`add_tag` (mirroring `projects::valid_project_prefix`'s approach), or
render tags through `yaml_scalar`/proper YAML sequence serialization
instead of manual string joining.

### 5. ⚪ LOW — Minor information disclosure in error bodies

Several error paths (`config.rs`, `tokens.rs`, `projects.rs`) format
raw I/O errors including full file paths directly into API error JSON
(e.g. `format!("reading {}: {e}", path.display())`). On a loopback-only
server this is low severity, but if the deployment model's "tunnel"
exposure is used, these paths will leak local filesystem layout to
whoever holds a valid (or, per Finding 2, even no) token. Consider a
generic error message for API responses with the detailed path only in
server-side logs.

---

## Status assessment against the product's own roadmap

- The product-owner documents (`STATE.md`, `ROADMAP.md`) are candid and
  current — "frozen until smoke pass green", explicit open PRs, explicit
  decisions with owner sign-off dates. This is unusually good bookkeeping
  for a PoC and makes the project easy to audit.
- Scope discipline is real: `AGENTS.md` §5 ("Security baseline") already
  names several of the right invariants (no `unwrap` on untrusted input,
  path-traversal rejection, secrets never in tracked files) and the code
  mostly honors them — `git::Repo::resolve` and `serve::static_route` both
  correctly reject `..`/separator/path traversal with tests
  (`path_traversal_is_rejected` in `git.rs`).
- The one place AGENTS.md's own rule ("validate all untrusted input at the
  boundary … errors handled explicitly, never `unwrap`ped on external
  data") is in tension with the code is Finding 1 — not an `unwrap`, but
  exactly the kind of untrusted-boundary trust violation the rule is
  trying to prevent, just one level up (trusting a *field*, not a crash).
- HTTP-mode agent integration (decision 13) explicitly ships without TLS;
  this is documented and reported as an `unsupported-transport` outcome
  rather than failing silently, which is the right way to ship a known
  gap — but it does mean the advertised use case of pointing at a hosted
  endpoint like Mistral (STATE.md decision 13) doesn't work over `https://`
  without an external TLS-terminating proxy, which should be made more
  prominent in user-facing docs (currently only in a code comment).
- Test coverage is broad for a PoC: per-module unit tests plus
  `tissue-cli/tests/{e2e,edge,serve,api_write,settings_agent,pull_push,
  release,gui_review,dev_fixture}.rs`. None of them exercise the
  cross-actor trust boundary in Finding 1, which is the one gap worth
  closing before the "#3 dogfood switch" milestone the roadmap is gating
  on (ironically, dogfooding is explicitly blocked on "#159 token
  lifecycle" + "#149 verification" already — this review's Finding 1
  belongs in that same gate).

## Suggested priority order

1. Fix Finding 1 (actor/token binding) — this is the one item that
   contradicts the product's stated differentiator and should block the
   #3 dogfood switch same as #159/#149 already do.
2. Cap request body size (Finding 2) — small, mechanical fix.
3. Thread-per-connection for `serve` (Finding 3) — small, unblocks safely
   shipping more concurrent agent tokens per decision 10.
4. Tighten `tags` serialization before any feature exposes free-form tags
   (Finding 4).
5. Generic API error bodies (Finding 5) — polish, not urgent.
