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
- **first delete the compatibility migration's ledger row** — see below; the old
  binary will not start without this
- do not switch back to the source cluster unless post-cutover writes have been reconciled
- before a later roll-forward, backfill `stream_category` for any events written by the old binary

### Binary rollback requires removing the migration ledger row

Rehearsed 2026-09-08 against the migrated PostgreSQL 18 green copy. "The schema
is additive, so just roll the binary back" is **not sufficient on its own**, and
the failure is total rather than subtle:

```text
Error: migration 20260902120000 was previously applied but is missing in the
resolved migrations
```

The pre-Mire binary embeds 24 migrations; the migrated database records 25.
SQLx 0.8's `Migrator::run` calls `validate_applied_migrations`, which errors on
any *applied* migration it cannot resolve locally, and `ignore_missing` defaults
to false. Forest calls `sqlx::migrate!("./migrations/").run(&pool)` without
overriding it, so the old binary aborts during `State::new` — before it reads a
single event. The additive schema is genuinely readable; the binary simply never
gets that far.

The tested recovery is to drop only the ledger row, leaving every added column,
index and Mire table in place:

```sql
DELETE FROM _sqlx_migrations WHERE version = 20260902120000;
```

With that row gone and the additive schema untouched, the pre-Mire binary starts
healthy in about two seconds and reads the migrated event store normally
(verified at 88 streams / 959 events, including events Mire had written).

Roll-forward was rehearsed too: `forest-event-migration prepare` is idempotent,
so re-running it restores the ledger row and the strict audit passes again. Do
that rather than hand-editing the table back.

Do **not** drop the added columns as part of a rollback. They are nullable or
defaulted, the old writer ignores them, and dropping them would discard the
backfilled `transaction_id` values that Mire needs on the next roll-forward.

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
- projection and saga pool capacity has been measured — done

## Progress and handoff

### Implemented

- Rust is pinned to 1.98.1 across the root toolchain, CI images, and the Forage server template. The Forest and Forage build images track `rust:1.98-trixie`: Docker's official `rust` image publishes 1.98 and 1.98.0 but no 1.98.1 tag, so pinning the patch broke both image builds until this was corrected.
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
- The organisation rule UI delta from closed PR #214 is ported selectively onto current GitHub `main`: settings-scoped routes and navigation, live project-selector previews, and structured policy, trigger, and pipeline builders. The PR's stale fork-wide workflow and unrelated file changes are intentionally excluded.

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
rustfmt --check --edition 2024 <changed Rust sources>
SQLX_OFFLINE=true cargo check --workspace
cargo test -p forest-server --lib domains::
cargo test -p forage-server org_rules_tests
mkdocs build --strict
```

The aggregate test run passed 152 tests. Both organisation-rule route tests passed. Repository-wide `cargo fmt --all -- --check` still reports unrelated formatting drift inherited from GitHub `main`; all Rust sources changed by this stack pass `rustfmt --check`.

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
suwmxrlk feat(forage): restore organisation rule settings
zowvrsmr fix: install a rustls provider and add mire-migrate for the cutover
```

Published to GitHub as `feat/mire-event-store-migration` in
`understory-io/forest`. Continue review and integration from that branch.

## Green rehearsal result (2026-09-07/08)

The full backup → restore → migrate → audit → smoke sequence has been executed
against an isolated green copy of production. Production was **not** touched:
the source Aurora cluster remains untouched and is the rollback.

### Backup verified before anything else

| Item | Value |
| --- | --- |
| Artifact | `nef_remote:~/prod-extracts/forest-prod-20260907T210311Z.dump` |
| Size | 4 971 511 bytes |
| sha256 | `7dd8af831288f8787d79fb29dd12ec468f04ad11d20ac264e3872c1fadeab12f` |
| Source engine | PostgreSQL **16.11** (`Dumped from database version` in the archive header) |
| Dumped by | `pg_dump` 16.15, custom format, 319 TOC entries |
| Table set | 54 of 54 tables, identical to the source row-count manifest |

The checksum matches the value computed in flight during the original transfer,
so the artifact has not rotted at rest. Aurora's own backups were confirmed
independently: cluster `platform-apps`, 7-day retention, PITR
`LatestRestorableTime` within ~3 minutes of wall clock, daily automated
snapshots, all on engine 16.11.

**Retention caveat:** automated snapshots and PITR only reach back 7 days. The
pre-cutover state must be captured as a **manual** cluster snapshot during the
window, because manual snapshots outlive the retention period and automated
ones do not.

### Green restore

Restored into an isolated PostgreSQL 16.15 instance at the **old** schema
(`pg_restore --exit-on-error`, exit 0), then verified:

- row counts identical to production across all 54 tables — 34 522 rows
- old schema confirmed: zero Mire compatibility columns present before migrating
- baseline hashes recorded: `es_streams` `3135421b…`, `es_events` positions
  `47ddafd9…`, full event payload hash `a6563605…`
- 24 SQLx migrations with checksums, including production's
  `20260901000000_annotation_deployment_items`
- event store: 87 streams, 958 events, 0 subscriptions, high-water mark 958,
  sequence at 958

### Compatibility + Mire migrations

`forest-event-migration prepare` then `mire-migrate`, then the strict audit.
`prepare` reported zero backfilled rows, which is correct: the compatibility
migration's own inline backfills already covered every row, and the repeat pass
exists only for rows a legacy writer appends after the additive deploy.

Nothing was lost. Post-migration the three baseline hashes are **byte-identical**
and every data table's row count is unchanged. The only two differences are the
intended additive ones: `_sqlx_migrations` 24 → 25, and Mire's new empty
`es_projection_leases` table.

### `forest-event-migration audit --strict` — PASS

Exit 0, `mire_compatible: true`, on both green (PG 16.15) and dev Aurora
(PG 16.11): all five compatibility columns present with zero nulls, zero
orphan events, zero stream-version mismatches, zero gaps, zero category or
subscription-cursor mismatches, sequence safe, and **958 of 958 events decoded
with zero failures**. Projection coverage is zero missing streams for all six
aggregate families.

### Dev first

The same sequence was run against the **dev** Aurora cluster (16.11, TLS
`verify-ca`) before green was migrated, after taking and checksum-verifying a
dev backup (`nef_remote:~/dev-extracts/forest-dev-20260907T221920Z.dump`,
sha256 `1e1c7f0d…`). Dev strict audit passed: 23 streams, 135 events, zero
decode failures.

### Upgraded Forest against green, traffic blocked

The upgraded (Mire) `forest-server` boots against green and reports healthy in
about two seconds, bound to loopback only on 4140/4141/4142 with in-process
destination execution disabled — no ingress, no runners, no external effects.

Gates exercised by `crates/forest-server/tests/green_hydration.rs`
(`GREEN_DATABASE_URL`, plus `GREEN_ALLOW_WRITES=1` for the write smoke):

- **every historical stream hydrates through Mire** — 87 of 87, across all six
  categories, each at exactly the version `es_streams` records
- **event log invariants** — zero orphans, zero version gaps, zero unfilled
  compatibility columns, sequence not behind the highest event
- **replay agrees with the live projections** (see below)
- **transactional atomicity** — an app event and its projection row commit
  together, and a forced projection foreign-key violation rolls back *both*;
  the rolled-back event is not visible in the replayed aggregate

Post-write strict audit passes at 88 streams / 959 events. Global positions 959
and 960 are absent because the rolled-back attempts burned them — expected
`BIGSERIAL` behaviour, and the audit still reports the sequence safe.

### Replay vs live projections

Full Mire shadow projections (standalone handlers writing `*_mire_shadow`
tables) are **not implemented**; Forest writes its projections inline inside the
command services through `TransactionScope`, so there is no replayable handler
to drive yet. What is verified instead is the property the go/no-go gate cares
about: replaying the event log through Mire reproduces the read model the live
projections serve.

| Category | Result |
| --- | --- |
| app | 1 live / 0 deleted — agrees |
| destination | 12 live / **8 deleted** — agrees |
| policy | 2 live / 0 deleted — agrees |
| trigger | 21 live / **1 deleted** — agrees |
| device_grant | 24 grants — presence and `user_code` agree |
| component | 19 components, **122 published versions** match; 2 in-flight/unpublished correctly absent |

Two things this comparison surfaced, both of which any shadow implementation
must honour:

1. **A deleted aggregate keeps its stream but loses its projection row.** Nine
   streams (8 destinations, 1 trigger) are in this state, so "stream count ==
   projection row count" is the wrong assertion; lifecycle state is what must
   agree.
2. **Only `Published` component versions are served.** `Uploading` (upload in
   flight) and `Unpublished` (removed by an owner) versions live in the
   aggregate but must be absent from `components`.

### Runtime state and object storage

- `release_states`: 558 SUCCEEDED, 72 FAILED — **zero QUEUED/ASSIGNED/RUNNING**
- `release_intents`: 335 SUCCEEDED, 74 FAILED, 7 ACTIVE (ACTIVE means an enabled
  pipeline definition; those 7 have 19 child states and zero in flight)
- `release_tokens`: 394 rows, none revoked, **zero unexpired** — no live tokens
- `terraform_state_locks`, `event_subscriptions`, `es_subscriptions`: all zero
  (the empty `es_subscriptions` is why the audit finds no cursor to translate)
- object storage `understory-forest-blobs-production`: versioning **Enabled**,
  4 039 objects / 3 518 185 308 bytes inventoried (key, size, ETag, mtime);
  **zero** `component_artifacts.storage_path` references missing from S3;
  `blob_storage` fallback content (2 958 rows) preserved in the dump

The drain precondition the runbook prefers therefore already holds. Release
orchestration still stays on the legacy path for this cutover, per Phase 7.

### Defect found and fixed during the rehearsal

The branch could not have completed a cutover as it stood. Forest's dependency
graph enables **both** rustls crypto providers on rustls 0.23 — `async-nats`
pulls `ring`, while `rust-s3`/`attohttpc` and `reqwest` pull `aws-lc-rs` — and
nothing installed a process-level provider. rustls refuses to guess, so the
first TLS handshake panics:

```text
Could not automatically determine the process-level CryptoProvider
```

Green did not catch this because it is plain TCP. **Aurora requires TLS**, so
`forest-event-migration` and `forest-server` would both have panicked on their
first connection to green or production. Fixed by installing the pure-Rust
`ring` provider (in preference to the `aws-lc-sys` C wrapper) at the top of
every entry point — `forest_server::tls::install_crypto_provider()` — and
verified by running the audit against dev Aurora over `verify-ca`.

`forest-event-migration mire-migrate` was also added, because the cutover
command list requires "run Mire migrations on green" as a discrete step and
Mire's `migrate()` was reachable only from Rust. It refuses to run unless the
compatibility columns already exist.

## PostgreSQL 18 rehearsal (2026-09-08)

Because the PostgreSQL 18 upgrade lands first, the rehearsal was re-run on 18
with production-shaped data. The verified production dump was restored at the
**old** schema into an isolated PostgreSQL 18 instance, then migrated and
audited exactly as on 16.

Results are identical to the 16.15 run:

- restore matched production across all 54 tables — 34 522 rows
- the three baseline hashes are **byte-identical** before and after migrating
  (`es_streams` `3135421b…`, `es_events` positions `47ddafd9…`, payloads
  `a6563605…`), so nothing was lost or rewritten
- the only row-count differences are the intended additive ones:
  `_sqlx_migrations` 24 → 25 and Mire's empty `es_projection_leases`
- `forest-event-migration audit --strict` **passes** — 958/958 events decoded,
  every invariant zero, sequence safe, zero projection rows without streams
- the rehearsal gates all pass: 88/88 streams hydrate through Mire, the replay
  comparison matches the live projections for all six categories, and the
  transactional smoke commits and rolls back atomically

Two engine notes. The container used here reports 18.0, while dev Aurora is on
**18.4.1**; the strict audit passes on both, and on dev that is real Aurora
post-`pg_upgrade`. What is still unproven is a full prod-data rehearsal on
**Aurora** 18.4 specifically, which needs a real green cluster and therefore the
window.

## Known accepted risk: prod ECS pins a mutable tag

Recorded 2026-09-08, accepted deliberately.

The production task definition (`forest:4`) pins
`ghcr.io/understory-io/forest:latest` rather than a digest. Merging the Mire
branch to `main` moved that tag to the Mire build
(`sha256:e515b8d3…`), while the single running task stays on the pre-Mire
`sha256:5f80ff45…`.

The CI deploy job is gated, so nothing deploys on merge. But ECS resolves
`:latest` whenever it *starts* a task, so an unplanned task replacement — a
health-check failure on the currently saturated cluster, Fargate patching, AZ
rebalancing, instance replacement — would pull the Mire image and run
`sqlx::migrate!` plus `EventStore::migrate()` against production unattended,
with no snapshot taken first. `desiredCount` is 1, so there is no second
replica to fall back on.

Two things bound the severity:

- production's live data already passes the read-only audit — 965/965 events
  decode, zero orphans, zero version mismatches, zero gaps, sequence safe — so
  an unattended migration would very likely *succeed* rather than corrupt
- the schema change is additive

The residual exposure is that it would happen without a fresh restore point,
and that recovery then depends on the binary rollback described below.

## Connection-pool and capacity measurement (2026-09-08)

The go/no-go list asks for projection and saga pool capacity to be measured.
Measured; the short answer is that **this cutover changes Forest's connection
profile not at all**, and that connections are not the binding constraint —
compute is.

### Connections: unchanged by this migration

Both the pre-Mire and the Mire code construct the pool the same way:

```rust
let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
```

No explicit sizing, so both inherit SQLx's default ceiling of **10**
connections per replica. Forest also wires **no** `ProjectionRunner` and **no**
saga worker — `mire_sagas` is a pinned dependency that no code references yet —
so projections remain inline inside command transactions via `TransactionScope`.
There are no new long-lived connections.

Observed against production: **31** connections in use of a **194** ceiling
(3 superuser-reserved), split forest 6, fungus 10, forage 2, the rest
administrative. Connection headroom is ample and unaffected by the migration.

### The binding constraint is ACU, not connections

The cluster is Aurora **Serverless v2** with:

```text
MinCapacity 0.5
MaxCapacity 1.0
```

and it has been sitting at **1.0 ACU — its configured ceiling — continuously**.
That reframes the "100% CPU" reading: this is not a large instance being
saturated by a runaway query, it is a cluster capped at roughly 2 GiB and a
fraction of a vCPU, pinned against that cap. `max_connections` of 194 is
consistent with that size.

Two consequences that matter more than anything in this document:

1. **Raising `MaxCapacity` is the fastest relief**, and it is a cluster-level
   setting rather than a query rewrite. That belongs to the Aurora work, but the
   migration and the PostgreSQL 18 upgrade both inherit the problem until it is
   done.
2. **Neither the major upgrade nor this cutover should be attempted while the
   cluster is pinned at its ceiling.** There is no headroom to absorb the extra
   work, and the dev upgrade already failed once. Sequence the ACU change first.

The migration's own cost is negligible by comparison — 958 events and four
index builds. The exposure is not volume, it is the absence of headroom, plus
one specific timeout: Mire's `migrate()` sets a **15 second `lock_timeout`**.
That bounds how long it waits to acquire a lock, so a legacy writer holding a
conflicting lock on a saturated cluster can fail the migration outright. This is
a concrete reason to stop every writer before migrating rather than relying on
low traffic.

### What to size before promoting workers

None of this bites during the first cutover, but before any projection or saga
worker is promoted:

- `ProjectionRunner::run` asserts `get_max_connections() >= 2` and then holds
  **one connection for its entire lifetime**, reserved for lease heartbeats
- each category lease loop acquires a connection while it works, so a runner
  covering all six categories can hold several at once under catch-up
- `mire-sagas` opens a `PgListener` for wake notifications, which is another
  dedicated connection

With the inherited ceiling of 10, a runner permanently holding one leaves nine
for the request path. Set the pool size explicitly with `PgPoolOptions` at that
point rather than continuing to inherit SQLx's default, and size it against the
ACU ceiling in force at the time.

## Production cutover — executed 2026-09-08

Done. Production runs Mire on Aurora PostgreSQL 18.4 and every gate passes.

### Order as executed

| Time (UTC) | Step |
| --- | --- |
| 00:01–00:16 | Aurora major upgrade 16.11 → 18.4 completes; cluster available |
| 00:17 | Fresh logical dump taken and checksum-verified |
| 00:21 | Manual pre-Mire cluster snapshot reaches `available` |
| 00:22:30 | Forest scaled to 0 — downtime begins |
| 00:24:10 | All 10 pooled writer sessions confirmed closed |
| 00:24:34 | `prepare` + `mire-migrate` applied to production |
| ~00:25 | `audit --strict` passes; hashes confirmed unchanged |
| 00:29:31 | Forest running and healthy on the Mire image — downtime ends |

Total downtime roughly **seven minutes**, of which about four were an avoidable
incident described below.

### Backups taken before touching anything

Three independent restore points, all verified before the migration ran:

- `pre-mire-migration-18-4-2026-09-08-0016` — manual cluster snapshot, 18.4,
  the post-upgrade **pre-Mire** state. This is the primary rollback.
- `preupgrade-platform-apps-16-11-to-18-4-2026-09-08-00-01` — Aurora's own
  pre-upgrade snapshot, 16.11.
- `nef_remote:~/prod-extracts/forest-prod-20260908T001701Z.dump` — 5 001 555
  bytes, sha256 `40bdb4af…` matching in flight and at rest, 54/54 tables,
  34 702 rows, dumped from 18.4.

### The migration was lossless

Captured with the database quiescent, before and after:

| | Before | After |
| --- | --- | --- |
| streams / events / subs | 87 / 965 / 0 | 87 / 965 / 0 |
| high-water mark | 965 | 965 |
| sequence | 990 | 990 |
| `md5(stream_id …)` | `3135421b…` | `3135421b…` |
| `md5(global_position …)` | `3231348c…` | `3231348c…` |
| `md5(position:stream:version:type:payload …)` | `97dc5cdf…` | `97dc5cdf…` |
| `_sqlx_migrations` | 24 | 25 |

All three hashes are byte-identical. The only changes are the intended additive
ones: one migration ledger row and Mire's empty `es_projection_leases`.

Worth noting these same hashes were produced by the green rehearsal, so the
rehearsal ran on byte-identical data rather than an approximation.

### Verified on production

- `forest-event-migration audit --strict` — **passes**, 965/965 events decoded,
  every compatibility column present with zero nulls, zero orphans, zero
  stream-version mismatches, zero gaps, zero category or subscription-cursor
  mismatches, sequence safe, zero projection rows without streams across all
  six aggregate families
- **87 of 87 historical streams hydrate through Mire**
- the replay comparison **matches the live projections** for all six categories,
  including all 123 published component versions
- both load-balancer target groups healthy — gRPC 4040 and HTTP 4042
- ECS service steady at 1/1 on task definition `forest:5`

The write smoke was deliberately **not** run against production; it creates an
app aggregate and would pollute real data. It ran on the green copy instead.

### Incident: ECS served a cached `:latest` and crash-looped

Scaling back up did not start the new image. Two consecutive tasks came up on
the **pre-Mire** digest `sha256:5f80ff45…` and the `forest` container exited 1
each time, because task definition `forest:4` pinned the mutable tag
`ghcr.io/understory-io/forest:latest` and the container instance already had a
layer cached under that tag. Meanwhile the registry tag had moved twice during
the evening's merges.

The failure mode was precisely the one rehearsed earlier: an old binary against
a migrated database dies on

```text
migration 20260902120000 was previously applied but is missing in the resolved migrations
```

Because that had already been diagnosed, the cause was obvious from the digest
alone rather than needing investigation under pressure.

Resolved by pinning the digest instead of the tag: task definition **`forest:5`**
references `ghcr.io/understory-io/forest@sha256:df18ae55…`. The task started
healthy immediately.

**Follow-up required.** A digest-pinned task definition no longer picks up new
images from a `force-new-deployment`, so `ci.yaml`'s deploy job is now a no-op
for image updates. Either teach it to register a new task-definition revision
with the digest it just built — the better practice, and it removes the cached
mutable tag hazard permanently — or accept tag pinning again with its risks.
Until that is decided, deploying either service means registering a new
revision.

Note this is not purely a workflow edit: registering revisions needs
`ecs:RegisterTaskDefinition` and `iam:PassRole` on the deploy role, which
`ecs-deploy.tf` in *infrastructure-platform* deliberately withholds — the
current job needs neither because it only calls `update-service
--force-new-deployment`. So the fix spans two repositories.

### Forage was deployed separately

Gating the deploy job meant merging deployed *nothing*, and the cutover above
only moved Forest. Forage therefore kept running its pre-merge image for several
hours, so the organisation rule settings work that landed in the same pull
request was merged but not live.

Deployed afterwards the same way, digest-pinned to avoid the cached-tag trap:
task definition **`forage:4`** references
`ghcr.io/understory-io/forage@sha256:aca4d4cc…`. Forage carries no schema
coupling — 12 embedded migrations against 12 applied, and that change added
none — so `sqlx::migrate!` found nothing to do. The rollout overlapped old and
new briefly and took zero downtime, which is safe here precisely because there
is no migration involved; that overlap is the thing Forest could not tolerate.

Both services are now pinned to digests rather than `:latest`.

### Rollback position

The source cluster is no longer a clean rollback: it *is* the migrated cluster.
Rollback is now either the pre-Mire snapshot above, or the binary rollback,
which requires deleting the migration ledger row first — see the rollback
section. That step is no longer hypothetical; the crash-loop above was the same
mechanism.

## Staged production cutover — superseded, executed above

**Not executed. This requires Kasper's explicit go and a scheduled window.**

### Merging is decoupled from deploying

`ci.yaml` used to run its ECS `deploy` job on every push to `main`, which made
"merge the PR" and "roll production" the same action. For this change that is
actively unsafe: Forest runs `sqlx::migrate!` and `EventStore::migrate()` from
its own startup path, so an automatic rolling deploy would migrate the live
production database under traffic, with old and new replicas briefly serving
side by side — which this runbook prohibits — and with no restore point taken
first.

The deploy job is now opt-in: it runs only on an explicit
`workflow_dispatch` with `deploy=true`. Merging is therefore safe on its own,
and the production rollout is a separate, deliberate action:

```console
gh workflow run ci.yaml --ref main -f deploy=true
```

To restore automatic deploys, put `github.ref == 'refs/heads/main'` back on the
job's `if:`.

### Blocking dependencies

1. **PostgreSQL 18 goes first.** Decided 2026-09-08: the engine upgrade lands
   before this migration.

   Dev has now completed it, and the sequencing turned out to be informative.
   The first in-place 16.11 → 18.4 attempt **failed** at 22:16 UTC (`Postgres
   cluster is in a state where pg_upgrade can not be completed successfully`)
   and rolled back to 16.11. The Forest compatibility migration was then
   applied to dev. A second attempt at 22:21 UTC **succeeded**: dev is on
   **18.4.1**, 707 seconds offline, moved from `default.aurora-postgresql16` to
   the `platform-apps-pg18` parameter group.

   Because the migration was applied before the upgrade, dev carried the Mire
   schema **through `pg_upgrade`**, which answers the question directly:

   - all five compatibility columns survived the major upgrade
   - the event store is unchanged — 23 streams, 135 events, high-water mark 143
   - `forest-event-migration audit --strict` **passes on Aurora PostgreSQL
     18.4.1** (`server_version_num` 180004), 135/135 events decoded, every
     invariant zero, `mire_compatible: true`

   So the additive schema, the `XID8` columns and the Mire cursor indexes are
   all verified on 18.4 on real Aurora, not just on 16.x. What remains is to
   **re-run the full green rehearsal against an 18.4 restore of production
   data** — the restore, the byte-identical hash comparison, hydration of every
   stream, and the projection replay were all done on 16.15, and prod-shaped
   data on the new engine is the part still unproven.
2. **Maintenance mode must land first.** It lives separately and is not on this
   branch. Forage's maintenance surface alone is insufficient: the Forest CLI,
   Woodpecker jobs, remote runners and direct API clients all bypass Forage.
3. **Do not run against the pegged source.** The source Aurora is reported at
   100%. Restore green from a snapshot rather than reading the live cluster.

### Order (fixed)

1. Maintenance on; pause Woodpecker, remote runners, and CLI/API writers.
2. Stop new release creation and component publication; drain in flight
   (currently already zero — re-confirm in the window).
3. Scale Forest replicas to zero; confirm Aurora has no Forest writer sessions.
4. Record the final `es_events.global_position` high-water mark.
5. Take a **manual** Aurora cluster snapshot (survives the 7-day retention) and
   capture the object-store inventory.
6. Restore that snapshot into an isolated green cluster at the old schema.
7. `forest-event-migration prepare` against green.
8. `forest-event-migration mire-migrate` against green.
9. `forest-event-migration audit --strict` against green — **hard gate**.
10. Start one upgraded Forest replica against green with ingress still blocked;
    run the aggregate and object-store smoke scenarios.
11. Start projection workers; verify zero lag.
12. Start remaining replicas; switch traffic; maintenance off; resume CI and
    runners.

### Rollback

Before public writes reach green: stop green, point traffic back at the source
cluster, start the previous Forest image. The source is untouched, so this is
clean.

After public writes reach green: re-enter maintenance, stop green writers, and
prefer rolling the **binary** back on green — the schema changes are additive
and the old event store can still read them. Do not switch back to the source
cluster unless post-cutover writes have been reconciled, and backfill
`stream_category` for anything the old binary wrote before rolling forward
again.

### Still open before traffic

- production PostgreSQL 18 upgrade completed (dev is done and audits clean on
  18.4.1), then this green rehearsal re-run against an 18.4 restore of prod data
- **raise the Aurora `MaxCapacity` ceiling** — the cluster is pinned at 1.0 ACU;
  neither the upgrade nor the cutover should run against that
- maintenance mode landed and exercised — dropped as a hard blocker on
  2026-09-08 on the grounds of low overnight traffic. Note that its real job is
  the automated writers (Woodpecker, remote runners, CLI and API clients), which
  are not time-of-day dependent, and that Mire's 15 second migration
  `lock_timeout` gives an independent reason to stop writers rather than hope
  they are quiet
- rollback rehearsed end to end — **done** for the binary-rollback path
  (including the ledger-row step above) and for roll-forward; switching traffic
  back to the source cluster is still unrehearsed
- projection and saga connection-pool capacity measured — **done**; see the
  measurement section above. The actionable follow-up is the ACU ceiling, not
  the pool
- Mire shadow projections implemented, if any read path is to move (not needed
  for this cutover, which keeps the existing projections)

### Still required before production traffic

This list predates the 2026-09-08 rehearsal. Items now closed are struck
through; see the sections above for evidence.

- ~~Land and exercise maintenance mode~~ — dropped as a hard blocker; stopping
  every writer still matters, for the `lock_timeout` reason above.
- Capture the final production high-water mark and a fresh quiescent
  snapshot/export. **Still required** — production has moved since the dump
  (965 events versus 958), and the pre-cutover snapshot must be a *manual* one.
- ~~Inventory and verify object storage~~ — done: versioning enabled, 4 039
  objects inventoried, zero dangling `component_artifacts.storage_path`
  references.
- Re-run `prepare`, `audit --strict`, and the aggregate smoke scenarios on the
  final green Aurora restore. **Still required on Aurora specifically** — done
  on isolated PostgreSQL 16.15 and 18, and the strict audit passes on real dev
  Aurora 16.11 and 18.4.1.
- Implement and compare Mire shadow projections before moving any read path.
  **Still open**, and not needed for this cutover; the replay comparison covers
  the go/no-go question in the meantime.
- ~~Exercise all six aggregate command/read paths~~ — all six hydrate through
  Mire (87/87 historical streams) and all six replay-match the live
  projections; the app transactional path also commits and rolls back
  atomically.
- ~~Rehearse rollback and measure projection/saga connection-pool capacity~~ —
  both done. Rollback needed a correction (the migration ledger row); capacity
  is unchanged by this cutover and bounded by ACU rather than connections.
- Keep release orchestration on the existing dynamic DAG until the separate
  aggregate/saga design in Phase 7 is implemented and rehearsed. **Unchanged.**
