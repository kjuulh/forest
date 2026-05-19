# Forage Client - Project Memory

## Project Overview
- Forage is a server-side rendered frontend for forest-server
- All auth/user/org management via gRPC to forest-server's UsersService
- No local user database - forest-server owns all auth state
- Follows VSDD methodology

## Architecture
- Rust workspace with 5 crates: forage-server, forage-core, forage-db, forage-grpc, ci
- forage-grpc: generated proto stubs from forest's users.proto (buf generate)
- forage-core: ForestAuth trait (async_trait, object-safe), validation, types
- forage-server: axum routes, gRPC client impl, cookie-based session
- MiniJinja templates, Tailwind CSS
- Forest + Mise for task running

## Key Patterns
- `ForestAuth` trait uses `#[async_trait]` for object safety -> `Arc<dyn ForestAuth>`
- `GrpcForestClient` in forage-server implements ForestAuth via tonic
- `MockForestClient` in tests implements ForestAuth for testing without gRPC
- Auth via HTTP-only cookies: `forage_access` + `forage_refresh`
- `RequireAuth` extractor redirects to /login, `MaybeAuth` is optional
- Templates at workspace root, resolved via `CARGO_MANIFEST_DIR` in tests

## Dependencies
- tonic 0.14 + tonic-prost 0.14 + prost 0.14 (must match for generated code)
- axum-extra with cookie feature for cookie management
- async-trait for object-safe async traits
- buf for proto generation (users.proto from forest)

## CI/CD
- Dagger-based CI in ci/ crate: `ci pr` and `ci main`
- `mise run ci:pr` / `mise run ci:main`
- Docker builds with distroless runtime

## Current State
- 20 tests passing (6 validation + 14 integration)
- Spec 001 (landing page): complete
- Spec 002 (authentication): Phase 2 complete
- Routes: /, /pricing, /signup, /login, /logout, /dashboard, /settings/tokens
- FOREST_SERVER_URL env var configures gRPC endpoint (default localhost:4040)
