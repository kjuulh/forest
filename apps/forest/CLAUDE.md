# Forest Development Guidelines

## Authorization

Every new gRPC endpoint MUST include authorization checks using `crate::grpc::authorize`. Before any business logic:

1. Call `authorize::extract_actor(&request)?` BEFORE `request.into_inner()`
2. Call the appropriate access check:
   - `authorize::require_org_access(db, actor, org_name, OrgRole::Member)` — for requests with `organisation: String`
   - `authorize::require_project_access(db, actor, project, OrgRole::Member)` — for requests with `project: Project { organisation, project }`
   - `authorize::require_org_access_by_id(db, actor, org_id, OrgRole::Member)` — for requests with `organisation_id: UUID`
3. For requests that only have a resource ID (e.g. `app_id`, `destination name`), look up the owning organisation first, then check access.

Use `OrgRole::Admin` for destructive org-level operations (member management). Use `OrgRole::Member` for everything else.

Service accounts bypass org checks (cross-org infra access). App tokens are auto-scoped to their organisation.

The authz acceptance tests in `tests/accepttest/authz_flow.rs` verify cross-org isolation. Add new tests when adding new resource types.

## Testing

- Tests use the dev database (`DATABASE_URL` from `.env`). There is no `clean_database` — tests must use unique names (UUID suffixes) to avoid collisions across runs.
- Never reset or wipe the database in tests.
- Run `cargo test -p forest-server` to execute all unit + acceptance tests.
- Run acceptance tests 3x to verify idempotency if you change test fixtures.

## Event-Sourced Aggregates

Domain aggregates live in `crates/forest-server/src/domains/`. Write operations go through aggregate commands + `save_with()` for atomic event + projection updates. Read operations query projections directly.

When adding a new aggregate:
- Domain logic (events, state, commands) in `domains/<name>.rs`
- Service orchestration in `services/<name>_aggregate.rs`
- Keep projection tables as-is (no schema changes needed)

## Build

- `SQLX_OFFLINE=true` for compilation without a live database (uses `.sqlx/` cache)
- After adding/changing SQL queries: `mise run db:prepare`. The offline cache the
  release build reads lives at `crates/forest-server/.sqlx/`. Do **not** use
  `cargo sqlx prepare --workspace` — it writes to the workspace root instead and
  leaves the real cache stale, so the query compiles locally (live `DATABASE_URL`)
  and then fails the Docker build, which sets `SQLX_OFFLINE=true`.
- To check the cache is current without rewriting it, from `crates/forest-server`:
  `cargo sqlx prepare --check`
- After adding/changing migrations: `sqlx migrate run --source crates/forest-server/migrations`
- Proto codegen: `buf generate`
