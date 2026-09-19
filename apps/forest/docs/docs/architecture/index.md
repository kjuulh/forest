# Architecture

An overview of Forest's internal architecture for contributors and operators.

## System Overview

```text
                         ┌─────────────┐
                         │  forest CLI │
                         └──────┬──────┘
                                │ gRPC
                         ┌──────▼──────┐
                         │forest-server│
                         └─┬──┬──┬──┬─┘
                           │  │  │  │
             ┌─────────────┘  │  │  └──────────────┐
             ▼                ▼  ▼                 ▼
        PostgreSQL          NATS forest-runner   destination adapter
                                                    │
                                      ┌─────────────┴─────────────┐
                                      ▼                           ▼
                                Forest Runtime          external provider /
                                                        customer platform
```

## Crates

| Crate | Purpose |
|-------|---------|
| `forest` | User-facing CLI tool |
| `forest-server` | Central backend (gRPC server, scheduler, coordinator) |
| `forest-runner` | Distributed execution agent |
| `forest-sdk` | Component SDK (traits, protocol) |
| `forest-sdk-codegen` | CUE → Rust/TS code generation |
| `forest-models` | Shared types (users, organisations) |
| `forest-grpc-interface` | Generated protobuf/tonic code |

## Event Sourcing

Forest uses Mire-backed event sourcing for its core domain aggregates:

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

The execution path depends on destination type:

- authenticated runners claim supported destinations and execute release work;
- built-in destination handlers currently execute from `forest-server`;
- `forage/containers@1` calls the managed runtime's `ForageService`;
- `forest/generic@1` calls an external
  `forest.provider.v1.DestinationProvider`.

Moving privileged destination work out of the control-plane process, adding
provider/runner workload identity, and reconciling target health independently
are productization gates.

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

Most user-facing gRPC handlers extract an actor from request metadata and apply
organisation or project checks. The current implementation is **not yet a
complete multi-tenant authorization boundary**:

- `RunnerService` is exempt from the normal authentication layer; registration
  accepts caller-supplied runner identity and capabilities before the scheduler
  can return scoped release credentials and destination metadata.
- Artifact publication records the actor when an upload begins, but subsequent
  upload, commit, abort, and retrieval operations rely on supplied identifiers
  without consistently rebinding the request actor to the original owner.
- The server-configured service-account credential has cross-organisation
  behaviour and requires replacement or strict scoping.

Treat the control plane as private trusted infrastructure until runner
authentication, artifact ownership checks, and the complete negative
cross-tenant matrix in the security plan are implemented and reviewed.

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
