# TASKS/024 — published versions are immutable (content write-once)

**Status:** spec / pre-implementation (VSDD Phase 1 — awaiting review)
**Created:** 2026-06-17
**Driver:** TASKS/023 caches a tool's `include` (env) map locally, keyed by
`(org, name, version)`, and loads it offline on the warm path without re-fetching
the manifest. That is only correct if a published `(name, version)` can never
change its content. Today it can: after `UnpublishVersion`, the same version
string may be re-published with *different* bytes. We want the registry to
guarantee that a **stable** version, once published, is permanently bound to its
original content — you bump the version to change anything. This is also the
behaviour developers already assume of a package registry (crates.io-style).

## Current behaviour (verified)

- The component aggregate (`crates/forest-server/src/domains/component.rs`) tracks
  per-version state `Uploading | Published | Unpublished` and records events
  `UploadStarted, FileUploaded, ManifestRecorded, VersionPublished,
  VersionUnpublished` (event log is permanent).
- `begin_upload` **rejects** a new upload when the version is already `Published`
  (`component.rs:163`, test `begin_upload_rejects_already_published_version`).
  So a published version is already effectively frozen — including identical
  re-publishes (a CI retry after a partial failure hits "already published").
- `UnpublishVersion` deletes the `components` projection row but keeps the
  `component_manifests` / `component_artifacts` rows and the event history; the
  version transitions to `Unpublished`, and `begin_upload` then **allows**
  re-publishing it with arbitrary new content
  (test `republish_after_unpublish_is_allowed`). ← the gap to close.
- Prereleases are detected by `is_prerelease(v)` (`component_aggregate.rs:69`):
  a `-` in the version core (after stripping `+build`). Currently prereleases are
  frozen-while-published like everything else. ← to be relaxed.
- A version is published as **one atomic manifest** (the rust/upload path emits a
  single-platform manifest; multi-platform goes through `prebuilt`/`external`
  which list all platforms in one manifest). There is no incremental
  per-platform re-publish of the same version, so "the whole manifest is the
  unit of immutability" does not conflict with any existing workflow.
- No manifest content hash is stored. Binaries are content-addressed
  (`component_artifacts.sha256`), and the manifest's `platforms` map embeds those
  same shas — so a hash over the manifest transitively pins the binaries too.

## Decisions (locked with the driver)

1. **Stable versions are content write-once.** Once a stable `(name, version)`
   has ever been published, a later publish of that version:
   - **succeeds as an idempotent no-op** iff the new content is identical to the
     originally-published content; else
   - **is rejected** with a "bump the version" error.
   This holds **even after unpublish** — a stable version is permanently *burned*
   to its first content. Re-publishing the identical content after an unpublish
   is allowed and **restores** the version (idempotent un-unpublish).
2. **Idempotent on byte-identical content** (CI-retry friendly) — see content
   identity below.
3. **Prereleases are mutable.** A version with a prerelease segment (`X.Y.Z-…`)
   may be re-published with different content, with or without an intervening
   unpublish. (Relaxes today's "frozen while published" for prereleases.)
4. **Burned forever.** The immutability record for a stable version survives
   unpublish and cannot be reclaimed for different content.

## Content identity

Two publishes of the same version are "identical" iff their **canonical manifest
hash** matches:

```
manifest_hash = sha256( canonical_json(manifest_json) )
```

- `canonical_json` = parse → re-serialise with lexicographically sorted object
  keys and no insignificant whitespace. Canonicalisation (not a raw-byte compare)
  guards against a newer CLI reformatting the JSON or reordering keys, which would
  otherwise look like a content change on an honest re-run.
- The manifest embeds every platform's binary `sha256`, so the manifest hash
  pins the binaries transitively; no separate artifact-hash comparison is needed.
- Rationale for "whole manifest": matches decision Q1 ("entire manifest
  immutable") and the existing one-atomic-manifest-per-version reality.

## Behavioural Contract

### B1. Classification
- `is_prerelease(version)` (existing helper) splits the world into **stable** and
  **prerelease**. All rules below apply only to **stable** versions; prereleases
  keep mutable/overwrite semantics.

### B2. Durable record
- The first `VersionPublished` for a stable version captures its
  `manifest_hash`. Persist it where it survives unpublish and DB-projection
  deletion:
  - Carry `manifest_hash` on the `VersionPublished` event (event log is the
    permanent source of truth), and
  - hold it in the aggregate state so command-time checks need no DB read:
    `versions: HashMap<String, VersionRecord { state, manifest_hash, prerelease }>`.
- Backfill: a migration computes `manifest_hash` for every existing
  `component_manifests` row (`sha256` of canonical JSON) so versions published
  before this feature are protected too. (Stored hash column optional for
  read-path/debug; the aggregate state is authoritative for enforcement.)

### B3. Publish gate (stable versions)
The enforcement point is **commit** (the manifest hash is only known after
`publish_manifest`), with `begin_upload` relaxed to permit re-staging so the hash
can be computed:

- `begin_upload` for a stable version that is `Published` or `Unpublished` in the
  aggregate ⇒ **allowed to stage** (no longer an immediate hard error). It must
  not clobber the live published manifest/artifacts until commit decides.
- At **commit_upload**, compute the new `manifest_hash` and compare to the
  recorded one for that version:
  - **equal** ⇒ idempotent success. No content changes. If the version was
    `Unpublished`, it is restored to `Published` (record a `VersionPublished`
    again; the projection row is re-inserted). If already `Published`, it is a
    pure no-op.
  - **different** ⇒ reject with
    `version <org>/<name>@<v> is immutable; it was published with different
    content — bump the version` and abort the staged upload (existing
    `AbortOnDrop` path).

### B4. Publish gate (prerelease versions)
- Prereleases may be (re)published with any content, whether `Published` or
  `Unpublished`. Overwrites the manifest/artifacts and updates the recorded hash.
  (No idempotency requirement — overwrite is the semantics.)

### B5. Unpublish
- `UnpublishVersion` unchanged in mechanics (delete projection, keep events), but
  the retained `manifest_hash` now enforces B3 on any later stable re-publish.

### B6. Authorization
- Unchanged: `OrgRole::Member` to publish/unpublish (parity with today).

### Edge Case Catalog
- **E1** Identical re-publish of a `Published` stable version ⇒ no-op success
  (CI retry). No new events beyond what idempotency requires.
- **E2** Identical re-publish after unpublish ⇒ restores to `Published`.
- **E3** Different re-publish after unpublish (stable) ⇒ rejected (burned).
- **E4** Prerelease re-publish (different content) ⇒ allowed, overwrites.
- **E5** Stable version never published before ⇒ normal first publish; hash
  recorded.
- **E6** CLI reformats/reorders manifest JSON between runs ⇒ canonicalisation
  makes it still "identical" (E1), not a spurious rejection.
- **E7** Two different versions sharing identical content ⇒ allowed; immutability
  is per-version, not global content-uniqueness.
- **E8** Backfilled pre-feature stable version ⇒ now immutable; a differing
  re-publish is rejected even though it predates the feature.
- **E9** Partial upload then abort, then re-stage identical ⇒ unaffected (no
  `VersionPublished` was recorded for the aborted attempt; commit is the gate).
- **E10** A stable version `Uploading` (in-flight) ⇒ existing supersede behaviour
  for the in-flight upload is unchanged; the gate is about *committed* content.
- **E11** Wasted binary upload: a genuine "same version, different content"
  mistake uploads a binary that commit then rejects. Acceptable; the error is
  clear. Optional client-side fast pre-check (see Risks R3).

## Verification Architecture

### Purity boundary
- **Pure core (unit/property tested):** `canonical_json` + `manifest_hash`;
  `is_prerelease`; the decision function
  `decide_publish(prev: Option<VersionRecord>, new_hash, prerelease) ->
  {FirstPublish | IdempotentNoop | Restore | RejectImmutable | OverwritePrerelease}`.
  This pure function is the heart of the feature and needs no DB.
- **Effectful shell:** event-store load/save, the commit transaction, projection
  writes, migration backfill.

### Provable / property-tested properties
- **P1 (write-once):** for a stable version with a recorded hash H, `decide`
  returns `Reject` for any new hash ≠ H and never `Overwrite`.
- **P2 (idempotence):** new hash == H ⇒ `IdempotentNoop` (or `Restore` if
  unpublished); applying twice is stable.
- **P3 (prerelease freedom):** prerelease ⇒ always `Overwrite`/allow, never
  `Reject`.
- **P4 (burned-after-unpublish):** unpublished stable + different hash ⇒ `Reject`.
- **P5 (canonical-hash stability):** `manifest_hash` is invariant under
  key-reordering and whitespace; never panics on any input that already passed
  `forest_manifest::parse`.

### Acceptance checks
- **A1:** publish stable `1.0.0`; re-publish identical ⇒ success no-op.
- **A2:** publish stable `1.0.0`; re-publish different ⇒ rejected.
- **A3:** publish `1.0.0`, unpublish, re-publish identical ⇒ restored & served.
- **A4:** publish `1.0.0`, unpublish, re-publish different ⇒ rejected (burned).
- **A5:** publish `1.0.0-rc1`, re-publish different ⇒ allowed, new content served.
- **A6:** backfilled pre-feature version rejects a differing re-publish.
- **A7 (regression):** the existing `republish_after_unpublish_is_allowed` test is
  re-pointed: allowed only for prereleases / identical content; the stable +
  different case now asserts rejection.

## Implementation Map (Phase 2 — not yet authorised)
1. **Aggregate** (`domains/component.rs`): add `manifest_hash` + `prerelease` to
   per-version record; thread hash into `VersionPublished`; add pure
   `decide_publish`; relax `begin_upload` stable gate to allow re-staging;
   implement restore-on-identical.
2. **Service** (`services/component_aggregate.rs`): compute canonical
   `manifest_hash` in `publish_manifest`; move the immutability decision into
   `commit_upload`; wire reject/idempotent/restore outcomes; keep `AbortOnDrop`
   semantics for rejects.
3. **Canonical hash helper**: shared pure fn (likely in `forest-manifest` so the
   CLI can pre-check too — Risks R3) for `canonical_json`/`manifest_hash`.
4. **Migration**: add `component_manifests.manifest_hash` (nullable), backfill via
   `sha256(canonical(manifest_json))`. (Or compute lazily on aggregate rehydrate
   if we prefer no schema change — see Open Question Q2.)
5. **Tests**: aggregate unit + property tests for `decide_publish`; re-point the
   regression test; accept tests A1–A7.
6. **CLI UX**: clear error on reject; optional `forest publish` pre-check.

## Risks / Open Questions
- **R1 — Restore-on-identical semantics. [RESOLVED]** Re-publishing identical
  content after unpublish *restores* the version to `Published` (clean idempotent
  undo of an accidental unpublish). Drives E2/A3.
- **R2 — Hash source of truth.** Enforce purely from the **event log / aggregate
  state** (no schema change), or also persist `manifest_hash` on
  `component_manifests` for read-path/debug + a cheap pre-check query?
  Recommendation: aggregate state is authoritative; add the column too (cheap,
  enables R3 and ops visibility).
- **R3 — Client pre-check.** Optional: `forest publish` asks the server for the
  stored hash and short-circuits to "already published (identical)" without
  uploading a binary, avoiding the wasted-upload case (E11). Nice-to-have.
- **R4 — Backfill correctness.** Canonicalisation of historical manifests must
  match the new canonicaliser exactly, or honest re-publishes of old versions
  would falsely reject. Backfill uses the *same* `canonical_json` routine.
- **R5 — Does anything legitimately overwrite a stable manifest today?** e.g. a
  re-`publish_manifest` within a single in-flight upload (the existing
  `ON CONFLICT DO UPDATE`). That intra-upload upsert stays; the new gate is only
  at commit against *previously committed* content. Confirm no tooling relies on
  mutating an already-committed stable version.

## Relationship to TASKS/023
This task **resolves 023 R1** for stable versions: the `(org, name, version)` env
cache can never be stale for a stable version, because the content is immutable.
For **prerelease** versions the env cache may be stale after an overwrite; that is
acceptable (prereleases are for iteration) and is refreshed by
`forest global update` / a cache clear. 023 should state this caveat.
