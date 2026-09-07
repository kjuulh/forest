# Mire migration runbook

This runbook replaces Forest's `forest-event-store` with Mire without losing PostgreSQL or object-store state. Forest runs on Aurora PostgreSQL; CockroachDB compatibility is not a requirement.

## Decision

Use an **in-place event schema upgrade on a restored Aurora copy**, not a second event log.

- Keep `es_streams`, `es_events`, and `es_subscriptions` as the canonical event tables.
- Add Mire's columns and indexes without rewriting or deleting existing events.
- Keep the existing projection tables during the first cutover so command responses retain synchronous read-after-write behavior.
- Build Mire shadow projections beside the live projections for replay comparison.
- Migrate release orchestration only after the generic event-store cutover is stable.
- Use a one-off migration command from the Forest image before starting the upgraded application.

Running two event logs side by side would require dual-write ordering, failure recovery, and conflict rules. It increases the number of states that can diverge and is rejected. Side-by-side **read projections** are safe because they are disposable and can be rebuilt from the canonical event log.

## Deployment topology

Use Aurora blue/green or snapshot-restore for rehearsal and cutover:

1. The source Aurora cluster remains the rollback source.
2. Restore the source snapshot or logical export into a green Aurora cluster at the **old Forest schema**.
3. Run the Forest migration job against green.
4. Run the audit and replay verification against green.
5. Start the upgraded Forest image against green while public traffic is still blocked.
6. Exercise internal smoke scenarios.
7. Switch traffic only after all gates pass.

If the extraction process produces a logical dump rather than an Aurora snapshot, restore/import the old schema and data first, then run migrations. Do not create the new schema first and import an old dump over it.

## Phase 1 — maintenance and quiescence

The maintenance-mode work currently lives separately and must land before the production cutover.

Maintenance is more than replacing the web page:

1. Enable the Forage maintenance surface.
2. Pause Woodpecker deployment jobs and other automated Forest clients.
3. Stop new release creation and component publication.
4. Let already-assigned releases finish where practical.
5. Stop remote runners or disconnect their completion channels.
6. Scale Forest server replicas to zero, or block Forest's gRPC/HTTP ingress and stop every background worker.
7. Confirm Aurora has no remaining Forest writer sessions.
8. Record the final `es_events.global_position` high-water mark.

Forage maintenance mode alone is insufficient because the Forest CLI, CI jobs, remote runners, and direct API clients can bypass Forage.

## Phase 2 — extract and restore

Capture all persistence layers while Forest is quiescent:

### Aurora

- Aurora cluster snapshot with point-in-time recovery retained through the migration window.
- Logical schema and data export for portable inspection.
- Database engine/version, schema migration list, table counts, primary-key hashes, and sequence values.
- Restore the backup into an isolated green cluster and prove it opens before changing the source.

### Object storage

- Bucket versioning state.
- Inventory containing key, version ID, size, checksum/ETag, and modification time.
- Cross-check component and artifact database references against object keys.
- Preserve `blob_storage` fallback content.

### Runtime state

Record active rows from:

- `release_intents`
- `release_states`
- `release_tokens`
- `terraform_state_locks`
- `event_subscriptions`

The preferred release migration has no `QUEUED`, `ASSIGNED`, or `RUNNING` releases. If work cannot be drained, release orchestration remains on the legacy implementation until a separate in-flight migration is rehearsed.

## Phase 3 — compatibility migration

Run the additive Forest SQL migration against green before the first Mire binary starts. It must:

- add and backfill `es_events.transaction_id XID8`
- add and backfill `es_subscriptions.last_transaction_id XID8`
- add and backfill `es_events.stream_category`
- add payload and metadata byte counts
- create Mire cursor/category indexes
- preserve every existing event and global position

After the compatibility migration, run Mire's own migrations. The compatibility migration must run first because Mire's initial migration creates an index on `transaction_id`; `CREATE TABLE IF NOT EXISTS` does not add missing columns to Forest's existing tables.

The migration is additive and remains readable by the old event-store binary. Mixed old/new Forest replicas are nevertheless prohibited.

## Phase 4 — prepare and audit

The migration image contains `forest-event-migration`. After every legacy writer
has stopped, run:

```console
forest-event-migration prepare
forest-event-migration audit --strict
```

`prepare` applies embedded Forest SQL migrations and repeats the category, size,
and subscription-cursor backfills. Repeating the backfill is required when the
additive schema was deployed before maintenance: the legacy writer can append
rows without Mire's denormalized category and byte counts.

The audit must fail on:

- Aurora/PostgreSQL older than 13
- missing Mire compatibility columns
- null compatibility values
- a legacy subscription position whose Mire transaction cursor points elsewhere
- event rows without streams
- stream versions that do not match their event history
- gaps or non-1-based stream versions
- event payloads that do not deserialize into the current aggregate event type
- stored `event_type` values that disagree with the deserialized event
- projection rows for the six aggregate families that lack an event stream
- a global-position sequence behind the highest event

Projection coverage is reported per aggregate category. Missing streams require
import/reconciliation events rather than deletion. Destinations are already
known to have possible legacy rows without streams; every aggregate family is
still audited and strict mode remains blocked until all are reconciled.

## Phase 5 — application upgrade

The first Mire application release performs a clean code cutover:

1. Upgrade Forest to Rust 1.98.1 and SQLx 0.9.
2. Pin `mire` and `mire-sagas` to an exact release or immutable revision.
3. Replace `forest_event_store::EventStore` with `mire::EventStore`.
4. Preserve all stream categories, stream IDs, event names, and JSON layouts.
5. Replace `save_with()` with Mire `TransactionScope` operations so event append and projection SQL remain one transaction.
6. Keep current projection tables as the read path.
7. Remove `forest-event-store` only after every caller has migrated.

Mire hydration is strict. Historical payloads that the old implementation silently skipped must be repaired with compatible legacy variants or explicit reconciliation events before traffic is enabled.

## Phase 6 — shadow projections

Run Mire transactional projection handlers from cursor zero into a separate schema or `*_mire_shadow` tables.

Required categories:

- app
- component
- destination
- device grant
- policy
- trigger

Compare stable IDs, lifecycle state, child records, token hashes/revocation, manifest references, and object keys. Do not compare volatile replay timestamps unless they are represented in the event payload.

Only promote an asynchronous projection where eventual consistency is acceptable. Request-critical read models may remain synchronously updated through Mire transactions.

## Phase 7 — release orchestration

Do not combine this with the generic event-store cutover.

Forest pipeline definitions are runtime `HashMap<String, StageDefinition>` DAGs. Mire Saga topology is fixed by `SagaBuilder` at startup, so a Forest pipeline cannot be translated directly into one Mire DAG without extending Mire.

The target split is:

- `ReleaseIntentAggregate`: owns the user-defined pipeline and stage states.
- `ReleaseAggregate`: owns one release lifecycle and replaces the manual `release_events` state machine.
- `ReleaseExecutionSaga`: a fixed per-release saga for policy gating, assignment, token issuance, execution, completion, notification, and parent-intent signalling.

Every external effect must deduplicate Mire's stable idempotency key. This includes runner assignment, destination execution, token issuance, notifications, and organisation events.

Historical `release_events` remain an immutable audit archive. Import streams contain the legacy event ID, sequence, actor, payload, and original timestamp.

## Cutover commands

The exact infrastructure wrapper may be Flux, Kubernetes Job, or an Aurora administrative runner, but the order is fixed:

```text
maintenance on
pause CI and external writers
drain releases
stop Forest and runners
capture high-water mark
snapshot/export Aurora and object storage
restore/import old data into green
forest-event-migration prepare against green
run Mire migrations on green
forest-event-migration audit --strict against green
start one upgraded Forest replica against green
run aggregate and object-store smoke scenarios
start projection workers and verify zero lag
start remaining replicas
switch traffic
maintenance off
resume CI and runners
```

## Rollback

Before public writes reach green:

- stop green
- switch traffic back to the source cluster
- start the previous Forest image

After public writes reach green:

- re-enter maintenance
- stop all green writers
- prefer rolling back the binary on green because the schema changes are additive
- do not switch back to the source cluster unless post-cutover writes have been reconciled
- before a later roll-forward, backfill `stream_category` for any events written by the old binary

After release sagas are enabled, the old release scheduler must never run concurrently with saga workers. Saga/outbox state must be materialized and reconciled before scheduler rollback.

## Go/no-go gates

Production traffic remains blocked until:

- the Aurora restore drill succeeds
- object-store inventory is complete
- the compatibility migration succeeds on production-shaped data
- `forest-event-migration audit --strict` passes
- every historical aggregate stream loads through Mire
- live and shadow projections match
- all six aggregate command/read smoke scenarios pass
- component and artifact objects can be fetched and hashed
- rollback has been rehearsed
- projection and saga pool capacity has been measured

## Progress and handoff

### Implemented

- Rust is pinned to 1.98.1 across the root toolchain, Forest and Forage build images, CI images, and the Forage server template.
- SQLx is pinned to 0.9.0 and the Forest server's offline query metadata has been regenerated.
- `mire` and `mire-sagas` are pinned to 0.3.0. Mire's optional NATS integration is not enabled; Forest's existing NATS dependency remains independent.
- All six aggregate contracts use Mire's `Aggregate`, `AggregateRoot`, `EventData`, and `RecordedEvent` types without compatibility aliases.
- Forest `State` owns `mire::EventStore`.
- Forest's transactional projection adapter uses Mire `TransactionScope`, so event append and synchronous projection SQL still commit or roll back together.
- The old `forest-event-store` crate, CI exclusions, test task, and fuzz task have been removed.
- `20260902120000_mire_event_store_compatibility.sql` creates the canonical event tables on a fresh database and additively upgrades legacy tables.
- `forest-event-migration prepare` applies Forest migrations and repeats the compatibility backfills under an advisory lock.
- `forest-event-migration audit --strict` checks PostgreSQL support, schema columns and nulls, stream integrity, sequence safety, subscription cursors, projection coverage, all 958 historical payloads, and stored event-type tags.
- The Forest image contains the migration binary and `mise` exposes preparation and audit tasks.
- Production had already applied `20260901000000_annotation_deployment_items.sql`; the exact migration and checksum are retained so SQLx accepts the production migration history.

### Production-data rehearsal

The production-derived logical dump is intentionally not committed. The local handoff artifact is:

```text
~/prod-extracts/forest-prod-20260907T210311Z.dump
```

Source counts are in `~/prod-extracts/source_rowcounts-20260907T210311Z.txt`. The event-store subset was 87 streams, 958 events, and zero subscriptions.

Before preparation, the audit decoded all 958 events with zero failures and found zero orphan events, version mismatches, or stream gaps. It correctly reported Mire incompatibility because Mire's additive columns were absent.

Preparation followed by strict audit passed on PostgreSQL 16.15, matching production's PostgreSQL 16 major version, and on PostgreSQL 18. The resulting schema had:

- zero null Mire compatibility values
- zero event-category or subscription-cursor mismatches
- zero projection rows without streams
- a safe global-position sequence
- 958 of 958 historical events decoded successfully

A focused cutover smoke scenario loaded `component-forest-contrib/build-rust` at version 16 through Mire, committed a new app event and projection atomically, then forced a projection foreign-key failure and confirmed both the event stream and projection rolled back. The post-write strict audit passed with 88 streams, 959 events, and zero decode failures.

Repository verification completed with:

```text
cargo fmt --all -- --check
SQLX_OFFLINE=true cargo check --workspace
cargo test -p forest-server --lib domains::
mkdocs build --strict
```

The aggregate test run passed 152 tests.

### Jujutsu checkpoint stack

Stable change IDs, oldest first, rebased onto `main@github` at `9529edb4`:

```text
mlqktsun feat: prepare Mire event-store migration
orqqrvnm chore: upgrade Forest to Rust 1.98.1
mmvrmkmn build: upgrade SQLx and pin Mire 0.3
nonrwpso refactor: replace Forest event store with Mire
ytzwzmys build: preserve production migration history
qxtomumn chore: upgrade Forage build images to Rust 1.98.1
omvlrukv docs: record Mire migration handoff
```

Published to GitHub as `feat/mire-event-store-migration` in
`understory-io/forest`. Continue review and integration from that branch.

### Still required before production traffic

- Land and exercise maintenance mode, then stop every Forest writer rather than only the Forage UI.
- Capture the final production high-water mark and a fresh quiescent snapshot/export.
- Inventory and verify object storage, including every component and artifact reference.
- Re-run `prepare`, `audit --strict`, and the aggregate smoke scenarios on the final green Aurora restore.
- Implement and compare Mire shadow projections before moving any read path.
- Exercise all six aggregate command/read paths; the focused rehearsal covered historical hydration and the app transactional path.
- Rehearse rollback and measure projection/saga connection-pool capacity.
- Keep release orchestration on the existing dynamic DAG until the separate aggregate/saga design in Phase 7 is implemented and rehearsed.
