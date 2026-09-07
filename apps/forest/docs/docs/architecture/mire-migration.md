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

## Staged production cutover — awaiting approval

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
- maintenance mode landed and exercised
- rollback rehearsed end to end
- projection and saga connection-pool capacity measured
- Mire shadow projections implemented, if any read path is to move (not needed
  for this cutover, which keeps the existing projections)

### Still required before production traffic

- Land and exercise maintenance mode, then stop every Forest writer rather than only the Forage UI.
- Capture the final production high-water mark and a fresh quiescent snapshot/export.
- Inventory and verify object storage, including every component and artifact reference.
- Re-run `prepare`, `audit --strict`, and the aggregate smoke scenarios on the final green Aurora restore.
- Implement and compare Mire shadow projections before moving any read path.
- Exercise all six aggregate command/read paths; the focused rehearsal covered historical hydration and the app transactional path.
- Rehearse rollback and measure projection/saga connection-pool capacity.
- Keep release orchestration on the existing dynamic DAG until the separate aggregate/saga design in Phase 7 is implemented and rehearsed.
