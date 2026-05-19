# 021: Publish ships the component's full file tree, not just .cue files

> Status: **SPEC — awaiting human review.** Per `../../spec.md`, no
> tests or implementation until this contract is signed off.

## Intent

`forest components publish` today only uploads `*.cue` files from the
component root (non-recursive) plus the compiled binary. Everything
else — `templates/`, schemas, scripts, README, static assets, hook
files — gets dropped on the floor. This breaks `forest release
prepare` for any consumer using a `version:` dep: prepare needs
`templates/deployment/{destination_type}/**` from the component on
disk, finds an empty cache dir, and bails with *"component must be
a local dependency"*.

The narrow fix would be "also publish `templates/`". The right fix
is to invert the default: **publish the component's full file tree**,
minus a sensible exclude set, with the publisher able to extend the
exclude set via a `.forestignore` file or pin an explicit allowlist
in `forest.component.cue`. Same idiom as `.gitignore` /
`.dockerignore` / `.npmignore`, plus the `cargo` / `npm` `include`
escape hatch when a publisher wants belt-and-braces.

After this lands, a consumer can keep using
`"understory/ecs-service": version: "0.1.0"` and `release prepare`
just works. The path-based workaround at the top of
`canopy-event-wall/forest.cue` (and any other consumer carrying the
same comment) goes away.

## Phase 1a — Behavioral Contract

### Publish (publisher-side change)

`forest components publish` walks the component directory and
currently uploads:
- The compiled binary (if present), via `upload_component_binary`.
- All `.cue` files in the component root, non-recursive, via
  `upload_component_file(rel_path, content)`. (See
  `crates/forest/src/cli/components/publish.rs:140-150` and the
  `collect_cue_files` helper at line 169.)

This is replaced by a generalised file walker. The new contract:

1. **Default = include everything** under the component root (the
   directory containing `forest.component.cue`).
2. **Default exclude set** (baked into forest, not configurable):
   - `.git/`
   - `target/` (Rust build output)
   - `node_modules/` (Node build output)
   - `cue.mod/pkg/` (vendored cue deps — re-vendored at consume time)
   - `.forestignore` itself
   - The compiled binary path resolved by `component_binary::resolve_binary`
     (uploaded separately as a typed binary, not as a generic file)
   - Files matching: `.DS_Store`, `.env*`, `*.swp`, `*.swo`, `*~`,
     `.idea/`, `.vscode/`
   - Symlinks (skip with a warning; we don't follow)
3. **`.forestignore` extends the exclude set.** Same syntax as
   `.gitignore` (gitignore-glob crate or equivalent). Lives at the
   component root. Optional. When present, its patterns add to the
   defaults — they never re-include something the defaults excluded.
4. **Optional explicit allowlist** in `forest.component.cue` via a
   new `paths` field on the component spec:

   ```cue
   forest: component: {
       name: "ecs-service"
       version: "0.1.0"
       paths: {
           // when set, ONLY paths matching one of these globs are
           // uploaded. Defaults + .forestignore still apply on top
           // (i.e. an include glob can't re-add a default-excluded
           // file).
           include: ["templates/**", "schemas/**", "README.md"]
       }
   }
   ```

   Absent ⇒ "include everything except the exclude set". Present ⇒
   the allowlist defines what's eligible; the exclude set still
   subtracts. This is the precedence we want — the SDK author's
   intent (allowlist) is sharper than the publisher's environment
   (`.forestignore`), but neither overrides the safety defaults.

**Path on the wire** is the component-rooted relative path with
forward slashes (e.g. `templates/deployment/forest/terraform@1/main.tf`,
`schemas/output.json`, `README.md`). The server-side `upload_file`
already accepts arbitrary `file_path` strings
(`crates/forest-server/src/services/component_aggregate.rs:104`)
and stores them in S3 + records a `component_files` row, so no
server change is required for upload.

**Path-traversal refusal:** any rel_path containing `..` segments,
absolute paths, or starting with `/` is rejected at publish time
with an error. Belt-and-braces.

**Caps** (guardrails against accidentally publishing binary blobs
or vendored deps that escaped the exclude set):
- Per-file cap: **10 MiB**, error on exceed (cited in the error so
  the publisher can decide: bump the cap by re-running with
  `--max-file-size`, fix `.forestignore`, or add the file to
  `paths.include` if they actually want it).
- Total-files-bytes cap: **50 MiB**.

Both are configurable via env vars (`FOREST_PUBLISH_MAX_FILE_BYTES`,
`FOREST_PUBLISH_MAX_TOTAL_BYTES`). Defaults match the values above.

**Determinism:** the file list is sorted by rel_path before upload,
so two publishes from the same source tree produce the same
upload-order. (Future: deterministic OCI layer hashing depends on
this.)

### Manifest

The manifest gains an optional `files` summary so consumers can tell
*"published with no extra files (intended)"* from *"published before
021"*. Both states have nothing in the cache beyond `.cue` files;
without the manifest field they're indistinguishable.

```json
{
  "name": "ecs-service",
  ...
  "files": {
    "count": 6,
    "total_bytes": 12345,
    "tree": [
      "README.md",
      "templates/deployment/forest/terraform@1/config.tf",
      "templates/deployment/forest/terraform@1/data.tf",
      "templates/deployment/forest/terraform@1/main.tf",
      "templates/deployment/forest/terraform@1/variables.tf",
      "schemas/output.json"
    ]
  }
}
```

`tree` is the full sorted list of rel_paths shipped (excluding cue
files, which are listed separately in the existing
`get_cue_files`-driven layer). For very large components this could
get long; cap it at 1000 entries and truncate with a `...` marker
at the end. Beyond 1000 files the `files.count` is the source of
truth.

### Server-side change

`get_cue_files` (component_aggregate.rs:261) currently filters
`file_path LIKE '%.cue'`. Add a sibling `get_non_cue_files` that
returns everything else, used at OCI packaging time so cue files
and "everything else" can be split into two layers if we want.

**Layer split decision:** one layer for cue, one for non-cue. Cue
is small and changes more often; non-cue is larger and tends to be
stable. Splitting maximises layer reuse across versions for
consumers that only bump the cue API. Cost: two more bytes in the
manifest, no real complexity. **Recommend split.** Confirm.

### Download (client-side)

`services/components.rs:343-360` streams component files via
`get_component_files` and currently keeps only `.cue` files (vendored
into `cue.mod/pkg/...` for cue-import). Two changes:

1. **Cue files** — keep vendoring into `cue.mod/pkg/forest.sh/...`
   as today (separate concern: import resolution).
2. **All files (including cue)** — write into the component cache
   directory at their original relative path, i.e.
   `~/Library/Caches/forest/components/{org}/{name}/{version}/{rel_path}`.
   Create parent dirs as needed.

The cache becomes a faithful mirror of the published file tree.
Cue files end up in *both* the cue.mod vendor dir (for type imports)
and the cache dir (for parity with local-component layout). They're
small and the duplication keeps the two consumer paths independent.

**Path-traversal check at download time too:** reject any rel_path
that contains `..` segments, is absolute, or attempts to escape the
cache dir, even if the server returned it. The publisher controls
that string at publish time and a malicious or buggy publisher must
not be able to write outside the cache.

After download, the cache directory layout for a registry-published
component looks like:

```
~/Library/Caches/forest/components/understory/ecs-service/0.1.0/
├── .forest/component/meta.json
├── forest.component.cue
├── forest.cue
├── spec.cue
├── README.md
├── schemas/
│   └── output.json
└── templates/
    └── deployment/
        └── forest/terraform@1/
            ├── config.tf
            ├── data.tf
            ├── main.tf
            └── variables.tf
```

This is the same layout `prepare` (and any other consumer that wants
to read a sibling file from the component) already expects of a
local dependency, which is why step 3 below is a one-line change.

### Prepare (consumer-side)

`crates/forest/src/cli/release/prepare.rs:184-189` currently bails
when the component source isn't `Local`. Replace with: when source is
`Versioned(version)`, resolve the component's cache directory
(`~/Library/Caches/forest/components/{org}/{name}/{version}`) and
treat it as `component_path`. The downstream code at
`prepare.rs:199-202` (`component_path.join("templates").join("deployment").join(destination_type)`)
then works unchanged, because the cache layout matches a local
component.

Edge case: if the cache dir has no `templates/` (e.g. component was
published before this task landed, or genuinely has no templates),
the existing `if template_dir.exists()` check at `prepare.rs:204`
already short-circuits — no error, no rendering, the deployment-item
just produces an empty output dir. We log a warning so the operator
isn't confused.

### Backwards compatibility matrix

| Component published before 021 | Component published after 021 |
|---|---|
| **Consumer using `version:`** (today: bails out) → still bails out, but with a clearer error: *"component {org}/{name}@{version} was published before forest 021; ask the publisher to re-run `forest components publish` after upgrading"*. | **Consumer using `version:`** → works. Cache is a full mirror; prepare reads templates directly. |
| **Consumer using `path:`** → works as today (path-based, doesn't touch the registry). | **Consumer using `path:`** → works as today. |

**Detection rule:** the manifest's new `files` field is the marker.
- Absent ⇒ pre-021 component. If the deployment-item needs templates
  for this destination_type, prepare returns the "too old" error
  pointing at the publisher.
- Present ⇒ post-021 component. Any of the listed files is in the
  cache; prepare proceeds normally. If `files.tree` is missing the
  required `templates/deployment/{destination_type}/` subtree,
  prepare emits the *existing* "no templates for this destination
  type" warning and produces an empty output dir.

## Phase 1a — Edge Case Catalog

| # | Scenario | Expected behavior |
|---|---|---|
| 1 | Publish a component with no extra files (just cue) | Manifest has `files.count: 0` (or absent `tree`). Cache after download contains only `.cue` files. Prepare emits *existing* warning for destination_types that need templates. |
| 2 | Publish with `templates/deployment/forest/terraform@1/*.tf` and a `README.md` | All files uploaded with their relative paths. Manifest `tree` lists them sorted. Prepare renders templates correctly; consumers can also read the README from the cache dir. |
| 3 | Publish with `*.jinja2` templates | Uploaded as-is. Prepare's existing `templates_service` flow renders them. |
| 4 | Publish with a 12 MiB blob accidentally checked in | Per-file cap fails publish with an error naming the file and the cap. The publisher fixes via `.forestignore` or `--max-file-size` override. |
| 5 | Publish with 60 MiB total | Total-bytes cap fails publish with a comparable error. |
| 6 | Publish with a symlink anywhere in the tree | Skipped with `tracing::warn!`. Never followed. |
| 7 | Publish with `.DS_Store`, `.env.local`, `target/release/...` in the tree | All skipped by the default exclude set, no `.forestignore` needed. |
| 8 | `.forestignore` with `dist/` pattern | Anything under `dist/` skipped at publish. The default excludes still apply. |
| 9 | `paths.include: ["templates/**"]` in `forest.component.cue` | Only files matching `templates/**` are eligible. Defaults still subtract (e.g. a hypothetical `templates/.DS_Store` is still skipped). |
| 10 | `paths.include: ["templates/**"]` AND a `.forestignore` containing `templates/old/**` | Allowlist gates eligibility, `.forestignore` subtracts. `templates/old/**` doesn't ship. |
| 11 | A file is mentioned in `paths.include` but doesn't exist | `tracing::warn!`; do not fail publish (publishers iterate; no-match globs are common during refactors). |
| 12 | A `.forestignore` pattern targeting `cue.mod/` | Already in default excludes; pattern is redundant but harmless. |
| 13 | A `.forestignore` pattern attempting to *re-include* a default-excluded path (e.g. `!target/dist`) | Ignored. Defaults are non-overridable. Document this; no UI to toggle. |
| 14 | Path with `..` in rel_path (e.g. via a clever symlink-to-skipped-dir-then-replace race) | Refused at publish; logged. Same check at download time as belt-and-braces. |
| 15 | Download a component that has files | Cache populated at the relative paths from the manifest's `files.tree`. Re-download is idempotent (write overwrites; no transactional dance). |
| 16 | Download a component published *before* this task | No `files` field. Cache populated only with cue files (today's behavior). Prepare emits the "too old" error if the destination_type needs templates. |
| 17 | Prepare with `version:` dep, templates flow through | Identical output to today's `path:` deps. Acceptance test compares snapshot dirs. |
| 18 | Prepare with `version:` dep where the component shipped no `templates/deployment/{destination_type}/` for the requested destination | Log warning, produce empty output dir, do not bail. Same as today's `path:` behavior. |
| 19 | Concurrent `prepare` runs against the same cache | Cache is content-addressed by `(org, name, version)`. Concurrent identical writes are at worst a wasted overwrite, never corruption. |
| 20 | Cache dir partially populated (interrupted download) | Re-download overwrites files. No transactional download in this task; cache converges on next run. |
| 21 | Re-publish the same version with different files | Re-publish is rejected today (immutable versions). No change. |
| 22 | Component is published with a 100k-file `tree` (someone forgot `.forestignore` for `node_modules/`) | Total-bytes cap kicks in long before the count cap matters. Default `node_modules/` exclude prevents the most common case anyway. Truncation marker `...` in `tree` after 1000 entries; `count` stays accurate. |

## Phase 1a — Non-Functional Requirements

- **Performance.** Templates are small text files; upload time is
  dominated by gRPC round-trip overhead per file. For typical
  components (≤20 files) the publish-time delta is sub-second.
- **Atomicity.** Upload is per-file; the existing `commit_component_upload`
  step is what makes the whole set live. Partial uploads on failure
  leak staging rows, same as today's behavior for cue files.
- **Cache hygiene.** No new cache eviction story in this task. The
  cache grows as components are downloaded; existing manual `rm -rf
  ~/Library/Caches/forest/components` remains the operator escape
  hatch. We do not introduce TTLs or LRU.
- **Backwards compatibility.** `path:`-based consumers keep working
  unchanged. `version:`-based consumers of components published
  *before* this task still see a clear error (with a different
  message than today) directing them to ask the publisher to
  re-publish.

## Phase 1b — Verification Architecture

**Pure / effectful split.**

- Pure: `component_walk(root, default_excludes, forestignore, allowlist)
  -> WalkResult { include: Vec<RelPath>, skipped: Vec<(RelPath, SkipReason)> }`.
  Walks the tree (against an injected fs trait so tests use
  `tempfile`), applies precedence:
  1. Default excludes (non-overridable).
  2. `.forestignore` patterns (subtract more).
  3. `paths.include` allowlist (if present, only-allowed; defaults
     and `.forestignore` still subtract).
  Then enforces per-file and total-bytes caps. Returns deterministic
  sorted output. Trivially fuzzable.
- Pure: `manifest_files_summary(walk_result, tree_cap=1000) ->
  serde_json::Value`. JSON construction with truncation.
- Effectful: the publish loop (uploads each file via gRPC), the
  download loop (writes each file under the cache root), the prepare
  lookup (joins cache root with component coords).

**Provable properties (test-level):**

- ∀ component root, `component_walk` returns a list sorted by
  rel_path so that publish ordering is reproducible across machines.
- ∀ component root with only the default excludes hit, the walk
  returns an empty result without erroring.
- The skip rules are total: every directory entry either ends up in
  `include` or in `skipped` with a `SkipReason`. No silent dropping.
- Default excludes are non-overridable. A `.forestignore` of
  `!target/` does not re-include `target/`.
- A path containing `..` after normalisation is never present in
  `include`, even if `.forestignore` or `paths.include` would
  otherwise admit it.

## Phase 2 — Test plan (to be written in Phase 2a, not now)

### Unit tests — `component_walk`

- Empty component dir → empty include set.
- Only `forest.cue` → one entry.
- `templates/deployment/forest/terraform@1/main.tf` plus `README.md` →
  both entries, sorted, rel_paths verbatim.
- `target/release/foo` → skipped via default exclude (not even read).
- `node_modules/big-pkg/index.js` → skipped via default exclude.
- `.env.local` → skipped via default exclude.
- `.forestignore` containing `dist/**` ⇒ files under `dist/` skipped.
- `.forestignore` containing `!target/dist` ⇒ no effect (default
  exclude wins).
- `paths.include: ["templates/**"]` ⇒ only files matching the glob.
- `paths.include: ["templates/**"]` + `.forestignore: templates/old/**`
  ⇒ allowlist gates eligibility, ignore subtracts.
- File >10 MiB → walker errors (caller can decide whether to fail
  publish or report).
- Sum of files >50 MiB → walker errors.
- Symlink → reported as skipped with `SkipReason::Symlink`. Never
  followed.
- Path normalisation: a publisher's `paths.include` of `../etc/passwd`
  is rejected at parse time.
- Deterministic ordering across calls (sort by rel_path).

### Integration test — publish + prepare round-trip (`tests/component_publish_files.rs`)

End-to-end against the dev fixture:

1. Build a tiny test component on disk with `templates/deployment/forest/terraform@1/main.tf`,
   a `README.md`, and a `target/debug/junk` (which must be excluded).
2. `forest components publish` it.
3. Assert: server's `component_files` table for that version has the
   templates and the README, but **not** `target/debug/junk`.
4. In a separate test project, declare it as `version: "..."` and run
   `forest release prepare`.
5. Assert: `<deployment_output>/<env>/<dest>/<dtype>/main.tf` matches
   the source.
6. Assert: cache dir contains both the templates and the README at
   their rel_paths.

This test exercises the full pipeline + the default-exclude rule.
It's the load-bearing one.

### Acceptance test — backwards compat (`tests/component_publish_no_files.rs`)

Publish a component whose only files are `.cue`. Manifest carries
`files.count: 0`. Consumer with `version:` deps that doesn't need
templates for its destination_type works. No "old artifact" error.

### Acceptance test — `.forestignore` and `paths.include`

Two small components: one with `.forestignore`, one with
`paths.include`. Verify the published file set matches the precedence
rules and that the right things end up in the cache.

## Phase 3 — Adversarial review checklist (filled in after impl)

Likely Adversary hits to pre-empt:

- Does the publish loop upload files in a deterministic order?
  (Otherwise re-publishes diff for the wrong reason.) Spec says yes.
- Is the cache write atomic per-file? `tokio::fs::write` is
  open-truncate-write; not atomic against interrupted runs. Acceptable
  for a content-addressed store keyed on (org, name, version) — a
  re-download overwrites. Document the limitation; don't add tempfile
  + rename in this task.
- Does the download loop preserve relative paths verbatim, or does it
  apply normalization that could escape the cache dir? The server-
  stored paths are operator-controlled at publish time. **Reject any
  path containing `..` or starting with `/`** at download time, even
  if the server returned it. Belt-and-braces against a malicious or
  buggy publisher.
- Same path-traversal check on the publish side — strip `..`
  components in the walked rel_path; refuse to upload anything that
  ends up outside `templates/`.
- Is the manifest's `templates` field optional? Yes — publishers and
  consumers running pre-021 forest don't know about it; the JSON
  parse must tolerate its absence.

## Resolved decisions

1. **Default = include everything; `.forestignore` extends excludes;
   optional `paths.include` allowlist for explicit control.** Hybrid
   model lets typical components ship with no extra config while
   giving security-conscious authors an escape hatch.
2. **Default excludes are non-overridable.** No `!pattern` re-include
   from `.forestignore`. Keeps the foot-guns out (e.g. you cannot
   accidentally publish `.env`).
3. **Cache layout = local-component layout.** Files land at the same
   relative paths a local component would have, so prepare needs
   only a one-line change. Other tools that may want to read sibling
   files from a component (schemas, READMEs) get the same surface.
4. **No retroactive backfill.** Components published before this task
   stay broken-for-`version:`-deps until their publishers re-run
   `forest components publish` after upgrading. Error message names
   the publisher's action explicitly.
5. **Walk the whole tree (minus excludes), not just `templates/`.**
   The bug is general, not template-specific. We pay one re-publish
   to fix it for all future cases at once.
6. **Cap defaults: 10 MiB per file, 50 MiB total.** Operator-overridable
   via env. Sized so accidentally-checked-in binaries fail loudly,
   intentionally-large content needs a one-flag opt-in.
7. **Two OCI layers (cue, non-cue).** Layer reuse for consumers that
   only bump the cue API. Negligible complexity cost.

## Open question for the human

- **`paths` field placement in cue.** Putting `paths.include` on
  `forest.component` is the obvious home, but it conflates "what's
  the component" with "what's published". The alternative is a
  separate top-level `forest.publish.include` block. Recommend
  `forest.component.paths` because the publish-time file set is part
  of the component's identity (consumers see exactly what was
  shipped). Confirm.

## Rollout

1. Land this PR with the publish/download/prepare changes.
2. Re-publish each component under your control
   (`understory/ecs-service`, `forest-contrib/terraform-service`,
   etc.) at their next bump. Add `.forestignore` files where the
   default excludes aren't enough (rare).
3. Update `canopy-event-wall/forest.cue` (and any other consumer
   that's currently working around this) to use `version:` instead
   of `path:`. Drop the workaround comment.
4. Optionally: a follow-up grep across all consumers for inline
   `path:` workarounds that this fix obviates.
