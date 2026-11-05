# ADR-002 — `tissue serve` = core + API + embedded UI; UI is API-only and may run standalone

Date: 2026-11-05. Status: accepted (owner-signed). Supersedes: none; refines decision 5/7 in `operations/product-owner/STATE.md`.

## Context

`tissue serve` today is one process on loopback serving the REST API **and** the statically embedded GUI. The owner asked whether serve should be core+API only, with the front-end as a separate service, and ruled: keep the embedded GUI for the simple single-command launch/dev-test path, but make sure it can be split later.

## Decision

1. `tissue serve` remains **core + REST API + embedded UI** in one binary — the default and only *required* deployment shape. The single-box deploy (one systemd unit, one auto-upgrading binary, loopback + SSH tunnel) stays as is; no second required service is ever added (roadmap "explicitly not now" stands).
2. The UI is a **pure API client** and must stay that way: it renders only from `/api/*` responses, holds no business logic, and keeps no state the API doesn't have. Today that is `src/gui/app.ts`; if the UI is reimplemented (see ADR-003 candidate: server-rendered HTML or WASM front-end), the same rule applies.
3. The UI code must be **splittable into a standalone process** later (`tissue-ui --api http://127.0.0.1:7323` or equivalent) without API changes — the embedded mode is just the standalone mode with the API base URL implied and the static files served by the same process.
4. Auth for a standalone UI: the browser holds the token (sessionStorage today); a standalone UI process never sees the serve token itself — it proxies nothing; the browser talks to the API directly. This keeps the token boundary unchanged.

## Consequences

- Positive: zero operational cost today, clean exit path to a separate front-end service, API stays the single contract.
- Negative: none for the single-box deploy; a future split needs its own deploy story (documented when it happens).

Refs #1, STATE.md decisions 5/7
