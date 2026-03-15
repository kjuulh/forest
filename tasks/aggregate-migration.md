# Aggregate Migration Status

## Completed

### Trigger — fully event-sourced
- `domains/trigger.rs` — aggregate with 24 unit tests
- `services/trigger_aggregate.rs` — writes + reads (replaces old `services/trigger.rs`, deleted)
- `grpc/triggers.rs` — all CRUD + evaluate routed through aggregate service
- `grpc/release.rs` — trigger evaluation uses aggregate service

### Policy — writes event-sourced, reads via projection
- `domains/policy.rs` — aggregate with 16 unit tests
- `services/policy_aggregate.rs` — create/update/delete through aggregate
- `services/policy.rs` — kept for evaluate, approval decisions, listing (projection reads)
- `grpc/policies.rs` — writes routed through aggregate, reads through PolicyRegistry

## Architecture Notes
- Using `save_with()` for synchronous inline projections (events + projection in same tx)
- This is intentional: one projection per aggregate, immediate read-after-write consistency
- Org event bus handles decoupled fanout to external consumers
- Event log in `es_events` supports future replay/rebuild if needed
- Consider adding a `rebuild-projection` admin command if projection drift becomes a concern

### App — writes event-sourced, reads via projection
- `domains/app.rs` — aggregate with 11 unit tests
- `services/app_aggregate.rs` — all writes through aggregate
- `services/apps.rs` — trimmed to type definitions only
- `grpc/apps.rs` — all operations through aggregate service
- Token hash stored only in projection (never in events)

## All migrations complete
- 108 unit tests, 9 acceptance tests passing
- Every domain aggregate uses `save_with()` for atomic event + projection writes
