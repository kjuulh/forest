# Architecture

An overview of Forest's internal architecture for contributors and operators.

## System Overview

```
                    ┌─────────────┐
                    │  forest CLI │
                    └──────┬──────┘
                           │ gRPC
                    ┌──────▼──────┐
                    │forest-server│
                    └──┬───┬───┬──┘
                       │   │   │
              ┌────────┘   │   └────────┐
              ▼            ▼            ▼
         PostgreSQL      NATS     forest-runner(s)
```

## Crates

| Crate | Purpose |
|-------|---------|
| `forest` | User-facing CLI tool |
| `forest-server` | Central backend (gRPC server, scheduler, coordinator) |
| `forest-runner` | Distributed execution agent |
| `forest-sdk` | Component SDK (traits, protocol) |
| `forest-sdk-codegen` | CUE → Rust/TS code generation |
| `forest-event-store` | Generic event store library |
| `forest-models` | Shared types (users, organisations) |
| `forest-grpc-interface` | Generated protobuf/tonic code |

## Event Sourcing

Forest uses event sourcing for its core domain aggregates:

- **Events** are immutable facts stored in append-only tables
- **Aggregates** reconstruct state by replaying events
- **Projections** are materialized views for fast reads
- **Commands** validate business rules and produce events

Pattern:

```
Command → Aggregate.apply(events) → save_with(events, projection_update)
```

Aggregates: `App`, `Component`, `Destination`, `Trigger`, `Policy`

## Release Lifecycle

Releases use a dedicated event store (`release_events` + `release_states` projection):

1. `create_release()` → Queued
2. Scheduler assigns to runner → Assigned
3. Runner executes → Running
4. Runner reports result → Succeeded / Failed
5. ReleaseReaper catches stuck releases → TimedOut

## IntentCoordinator (Saga Orchestrator)

For pipeline releases, the IntentCoordinator manages the multi-stage lifecycle:

- Subscribes to NATS `forest.intent.evaluate` + 5s polling fallback
- Idempotent evaluation: loads full state with `FOR UPDATE SKIP LOCKED`
- Activates stages when dependencies are satisfied
- Handles transitive cancellation
- Tracks `stage_states` JSONB as the saga's source of truth

## Scheduler

NATS-driven with 5s fallback sweep:

- Listens for `forest.release.queued` signals
- Picks up Queued releases, assigns to available runners
- Branches on release mode: `"plan"` runs prepare+plan, `"deploy"` runs prepare+release

## Authorization

Every gRPC endpoint enforces authorization:

1. Extract actor from request metadata (JWT / API key / app token)
2. Check org-level access (`require_org_access` / `require_project_access`)
3. Service accounts bypass org checks (cross-org infra access)
4. App tokens are auto-scoped to their organisation

## Event Bus

Organisation-scoped event streaming using the outbox pattern:

- `org_events` table (append-only) with BIGSERIAL sequence
- NATS `forest.events.{organisation}` for wake signals
- gRPC `EventService.Subscribe` for server-streaming to clients
- Transactional: release state changes write events in the same DB transaction

## Communication

| Channel | Purpose |
|---------|---------|
| gRPC | CLI ↔ Server, Runner ↔ Server |
| NATS | Async signaling (release queued, intent evaluate, event nudge) |
| PostgreSQL | Persistent state, event store, projections |

NATS is used as a signal layer only — all state is in PostgreSQL. If NATS is unavailable, fallback polling (5s intervals) ensures progress.
