# Migrate App to Event-Sourced Aggregate

## Status: Done

## What was done
- `domains/app.rs` — aggregate with 11 unit tests (create, suspend/unsuspend, delete, token create/revoke, hydration, serde)
- `services/app_aggregate.rs` — all writes through aggregate, reads from projection
- `services/apps.rs` — trimmed to just type definitions (AppInfo, CreatedAppToken, AppTokenInfo)
- `grpc/apps.rs` — all operations routed through aggregate service

## Design decisions
- Token hash stored **only in projection** (never in events) — raw token generated in service layer, hash written atomically with event via `save_with()`
- `revoke_token()` looks up `app_id` from `app_tokens` table since proto only sends `token_id`
- Suspend/unsuspend are idempotent — no event recorded if already in target state
- `create_token` rejects if app is suspended (domain guard)
