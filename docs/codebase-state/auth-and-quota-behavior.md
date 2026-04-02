# Auth And Quota Behavior

Snapshot date: 2026-03-13

## Behavior

- Authenticate clients and workers via shared bearer token.
- Enforce rate/concurrency/quota limits (not yet implemented).

## Status

- **Auth: partial** — stopgap shared bearer token implemented. No per-user tokens, no handshake.
- **Quota: not implemented.**

## Current implementation

### Bearer token auth (item 48)

- Axum middleware in `crates/prosthetic-conscience/src/router/auth.rs` checks `Authorization: Bearer <token>` header on all routes (`/ws/worker` and `/v1/chat/completions`).
- Gateway reads `PC_AUTH_TOKEN` env var at startup. If set, all requests require the matching token. If unset, auth is disabled (open access).
- Worker accepts `--auth-token <token>` CLI arg. If set, includes `Authorization` header on WS connect.
- Returns `401 {"error":{"message":"unauthorized"}}` on missing/wrong token.
- Tests run with auth disabled (no token configured in `TestGateway`).

## Invariants

- **Safety: no auth bypass when token is configured.** If `auth_token` is `Some`, every request without a valid `Authorization: Bearer <token>` header is rejected with 401 before reaching any handler.
- **Safety: auth disabled is explicit.** Auth is only disabled when `PC_AUTH_TOKEN` env var is unset (`auth_token: None`).
- **Liveness: open access when no token configured.** When `auth_token` is `None`, all requests pass through without auth checks.

## Load into context when

- Implementing auth token verification.
- Implementing quota/rate/concurrency checks.
- Implementing per-user tokens or handshake auth.
- Analyzing authorization or limit-enforcement behavior.

## Relevant files

- `crates/prosthetic-conscience/src/router/auth.rs` — middleware implementation
- `crates/prosthetic-conscience/src/router/mod.rs` — middleware wiring
- `crates/prosthetic-conscience/src/router/state.rs` — `AppState.auth_token` field
- `crates/prosthetic-conscience/src/bin/prosthetic-conscience.rs` — `PC_AUTH_TOKEN` env var reading
- `crates/prosthetic-conscience/src/worker/client.rs` — `WorkerClient.auth_token` field, `build_request()` header injection
- `crates/prosthetic-conscience/src/bin/pc-worker.rs` — `--auth-token` CLI arg
- Specification source: `docs/gateway-specification.md`

## TODO (near-term)

- Proper auth: per-user tokens, worker handshake with version/capability validation (item 43).
- Quota/rate/concurrency checks.
