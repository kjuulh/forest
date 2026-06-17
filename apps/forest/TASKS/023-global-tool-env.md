# TASKS/023 — component-declared default env for global tools

**Status:** spec / pre-implementation (VSDD Phase 1 — awaiting review)
**Created:** 2026-06-17
**Driver:** A global tool (e.g. `understory/fungus`, run as `fungus …` via the
PATH shim or `forest global run understory/fungus`) often needs a baseline of
environment configuration that is the same for everyone in the org — e.g. the
server it should talk to. Today `fungus` defaults to a *local* dev server, so a
fresh developer machine "tries to reach a local server" and fails until the
developer happens to know which env var to export. We want the tool's author to
declare, **once**, a set of default env vars that ship with the published
component and are auto-applied on every developer's machine — while still
letting any developer (or CI) override a value by simply having it set in their
own environment.

Concrete example: `fungus` should default `FUNGUS_SERVER` (or equivalent) to
the prod endpoint. If the developer has `FUNGUS_SERVER` exported in their shell,
that value is used unchanged.

## Problem

`forest global run <tool>` resolves the tool to a cached binary and `exec()`s it
with the parent process environment inherited verbatim
(`crates/forest/src/cli/global.rs:438-441`):

```rust
use std::os::unix::process::CommandExt;
let err = std::process::Command::new(&path).args(&self.args).exec();
```

There is no mechanism for a component to ship default env vars, and no layer
between "the developer's shell" and "the tool" where org-wide defaults can be
injected. The `user` kv map in `~/.config/forest/forest.cue` is parsed but never
applied at exec (`crates/forest/src/global/user_config.rs:17`).

## Design summary (decisions locked with the driver)

1. **The env vars are NOT part of the tool facet / describe protocol.** They are
   declared in an `include { … }` block on the component, sitting beside the
   binary upload block — conceptually "a bundle of things shipped next to the
   binary". `env` is the first member of `include`; the block is deliberately a
   container so future members (e.g. `files`, config blobs) can be added without
   another schema/manifest reshuffle. This avoids any change to the SDK,
   codegen, or the `_meta/describe` contract (`fungus` keeps emitting its
   current descriptor untouched).
2. **Declared in CUE, published into the manifest.** Changing the defaults
   requires a republish ("it requires a republish to get it out").
3. **Cached beside the binary, auto-loaded at run time.** When forest fetches a
   tool it persists the env map into the local cache; an env-map abstraction
   loads it from disk on every run, including fully offline (warm-path) runs.
4. **Defaults only — the ambient shell environment always wins.** Forest injects
   a var only if it is not already present in the process environment ("if
   overridden we accept the value provided").
5. **Per-tool local override, file-only.** A developer may override or extend
   the defaults by hand-editing their `~/.config/forest/forest.cue` per-tool
   dependency block. This is **not** settable via a CLI command. (No per-org or
   global local layer in this task.)
6. **Visible on the component page.** The declared env defaults are part of the
   published manifest and must be discoverable (`forest components show` and the
   registry component page).
7. **Scope: global tools only.** Project-scoped `forest run` is out of scope.

## Non-goals

- No change to `#ForestTool` / `ToolFacet` / `_meta/describe` / forest-sdk /
  forest-sdk-codegen. Env is orthogonal to the tool facet.
- No server-side org-config aggregate or new RPC. Distribution is via the
  existing published manifest (`manifest_json` is already stored and returned
  verbatim by the registry).
- No CLI command to set/override env locally (`forest global set-env …` is
  explicitly out of scope; overrides are hand-edited in `forest.cue`).
- No per-org / per-catalogue / global local env layer.
- No secret handling. These are plain-text defaults baked into a published
  manifest and visible on the component page — **not** a secrets mechanism.
  (Documented loudly; see Edge Cases.)
- No env injection for project `forest run`, hooks, or deployment templates.
- No "unset / delete an inherited env var" capability.

## Behavioural Contract

### B1. Authoring surface (CUE)

Add an optional `include` container to `#ForestComponent`
(`components/forest/sdk/spec.cue` and the mirror `cue/forest-sdk/spec.cue`),
with `env` as its first member:

```cue
// Artifacts shipped alongside the published binary and materialised into the
// local cache when the tool is fetched. A forward-looking container — `env` is
// the only member today; future members (e.g. `files`) slot in here without
// reshaping the manifest.
#ForestInclude: {
    // Default environment variables auto-applied (as defaults) when the tool
    // runs on a developer machine. Keys must be valid POSIX env names; values
    // are plain strings.
    env?: {[Name= =~#"^[A-Za-z_][A-Za-z0-9_]*$"#]: string}
    // (future) files?: [...#ForestIncludeFile]
}

#ForestComponent: {
    name:    string
    version: string & =~#"^\d+\.\d+\.\d+"#
    // ... existing fields (codegen?, upload?, external?) ...

    include?: #ForestInclude
}
```

Author usage in `fungus/forest.cue` (the project config — same block that holds
`upload`):

```cue
forest: component: sdk.#ForestComponent & {
    name:    project.name
    version: "0.1.9"
    upload: { source: "./crates/fungus", type: "rust", architectures: macos: arm64: {} }

    include: {
        env: {
            FUNGUS_SERVER: "https://fungus.understory.sh"
        }
    }
}
```

### B2. Manifest representation

`include` is emitted as a **top-level manifest field**, a sibling to `platforms`
(NOT nested under `tool`), mirroring the CUE block so future members ride along
unchanged. Example published manifest for a TOOL_BINARY:

```json
{
  "kind": "binary",
  "tool": { "name": "fungus", "argv_passthrough": true, "description": "…" },
  "methods": [],
  "include": { "env": { "FUNGUS_SERVER": "https://fungus.understory.sh" } },
  "platforms": { "darwin_arm64": { "sha256": "…", "size": 123 } }
}
```

- Emitted by every publish path that produces an installable tool: rust/binary
  upload (`publish.rs` binary path, ~`crates/forest/src/cli/components/publish.rs:422-461`),
  `prebuilt` (`publish_prebuilt`), `deno`, and `external` (`publish_external`).
  Source of the value is the CUE doc (`forest.component.include`), read directly
  — **not** the describe response.
- Absent/empty `include` ⇒ the manifest omits the field (or emits `{}`); both
  parse to an empty include (empty env map).
- The registry stores `manifest_json` verbatim and returns it on
  `GetComponentManifest` / `GetComponentDetail`, so env round-trips with **no
  server code change** (to be verified — see V-Check S1).

### B3. Manifest parsing

`forest-manifest::Manifest` gains an `include` field carrying a typed `Include`:

```rust
pub struct Manifest {
    pub kind: ManifestKind,
    pub tool: Option<ToolFacet>,
    pub methods: Vec<String>,
    pub include: Include,                // NEW — default (empty) when absent
    pub platforms: BTreeMap<PlatformKey, Platform>,
    pub shape: ComponentShape,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Include {
    pub env: BTreeMap<String, String>,   // empty when absent
    // (future) pub files: Vec<IncludeFile>,
}
```

Parsing rules (additive; pre-existing manifests without `include` stay valid):
- Missing / `null` `include` ⇒ default `Include` (empty env).
- `include` must be a JSON object; otherwise `ManifestError::InvalidJson`.
- `include.env` missing/`null` ⇒ empty map; otherwise must be a JSON object of
  string→string (`ManifestError::InvalidJson` on shape mismatch).
- Each env key must match `^[A-Za-z_][A-Za-z0-9_]*$` ⇒ else a new
  `ManifestError::InvalidEnvName(String)`.
- Each env value must not contain a NUL byte ⇒ else
  `ManifestError::InvalidEnvValue(String)`.
- Unknown members of `include` are ignored (forward-compat with older clients
  reading manifests that carry future include members).

### B4. Caching (persist beside the binary)

On the **cold path** in `GlobalService::resolve_to_cached_path`
(`crates/forest/src/global/service.rs:162-300`), after the manifest is fetched
and the binary is finalised, persist the manifest's `include` block to the local
cache so it is available on later **offline** (warm-path) runs.

- Cache location (new): a per-version include dir
  `~/.cache/forest/components/include/<org>/<name>/<version>/`, with the env map
  written as `env.json` (a JSON object of string→string). Future include members
  (e.g. files) materialise as siblings in this same dir, so the layout already
  anticipates the `include` container growing. Keyed by **(org, name, version)**,
  NOT by binary sha — see Open Question Q2 and Edge E7 (sha-dedup collision).
- Writes are best-effort-atomic (temp file + rename), same discipline as the
  lockfile writer.
- The warm path (lockfile hit + cache hit, no manifest fetch) loads the env from
  `env.json`. If it is absent (tool cached before this feature, or empty
  include) ⇒ empty map; the tool still runs.

### B5. Local per-tool override (user `forest.cue`)

Extend the per-tool `Dependency` block in `~/.config/forest/forest.cue`
(`crates/forest/src/global/user_config.rs`):

```cue
config: dependencies: "understory/fungus": {
    version: "0.1.9"
    env: { FUNGUS_SERVER: "http://localhost:8080" }   // NEW, optional
}
```

- Parsed into `Dependency.env: BTreeMap<String,String>` with the same
  name/value validation as B3.
- Hand-edited only. `forest global set` is unchanged; no new CLI command writes
  this. `render_user_config` (`service.rs`) must round-trip the field so other
  `forest global` mutations don't drop it.

### B6. Resolution & injection (the env-map abstraction)

A pure function computes the keys to inject:

```
resolve_injection(
    component_env: &Map,   // from cache (B4)
    local_env:     &Map,   // from user forest.cue dependency (B5)
    ambient_keys:  &Set,   // names present in std::env at launch
) -> Map  // exactly the (key,value) pairs to set on the child
```

Rules:
1. Start from `component_env`.
2. Overlay `local_env` (local key wins over component key).
3. Remove any key already in `ambient_keys`.
4. The result is the set of vars forest sets on the child.

Injection at exec (`global.rs` RunCommand): do **not** `env_clear()`; for each
`(k,v)` in `resolve_injection(...)`, call `cmd.env(k, v)`, then `exec()`. Because
ambient keys are removed in step 3, inherited ambient values are never
overwritten. `WhichCommand` is unaffected (it prints a path; no exec).

### B7. Precedence (lowest → highest)

1. Component-declared env (manifest → cache).
2. Per-tool local env (user `forest.cue` dependency block).
3. Ambient process environment (never overwritten).

### B8. Visibility

- `forest components show <org>/<name>` renders the declared `include.env` map
  (read from `manifest_json`, which already carries it). Pretty + JSON formats.
- The registry component page surfaces the same `include.env`. It reads from the
  returned `manifest_json`, so no proto change is strictly required (confirm the
  page's data source — Open Question Q3).

### Edge Case Catalog

- **E1** Key present in ambient env ⇒ never set/overwritten, regardless of
  source. (Core invariant.)
- **E2** Empty/absent component `env` and no local override ⇒ exec env identical
  to today (pure passthrough). Zero behaviour change for tools that don't opt in.
- **E3** Env cache file missing (tool cached pre-feature) ⇒ treated as empty;
  tool still runs; defaults appear after the next cold fetch (e.g. version bump).
- **E4** Local override adds a key not in component env ⇒ injected (subject to
  E1). Local override sets a key to `""` ⇒ injects empty value (subject to E1).
- **E5** Invalid env name at publish ⇒ rejected by the CUE constraint and by a
  publish-time check; defensively re-rejected by `forest-manifest::parse` at
  fetch (`InvalidEnvName`). Value containing NUL ⇒ `InvalidEnvValue`.
- **E6** Value with newlines / `=` / spaces / unicode ⇒ allowed (only NUL is
  forbidden); set verbatim.
- **E7** Two versions (or two components) sharing one binary sha but different
  env ⇒ correctly distinguished because the env cache is keyed by
  (org, name, version), not by sha.
- **E8** Manifest `env` is **not secret**. It is plain text in a published
  artifact and on the component page. A publish-time advisory should warn if a
  key name looks secret-ish (`*_SECRET`, `*_TOKEN`, `*_KEY`, `PASSWORD`,
  `*_PASS`); does not block. (Confirm — Open Question Q4.)
- **E9** Concurrent `forest global run` of the same tool ⇒ env cache writes are
  atomic-rename and idempotent (same version ⇒ same bytes); readers tolerate a
  transient missing file (treat as empty, E3).
- **E10** Hand-edited `forest.cue` with a malformed `env` (non-object, non-string
  value, bad name) ⇒ a clear `UserConfigError`; `forest global run` fails fast
  with the parse error rather than silently dropping the override.
- **E11** `external` (URL-hosted) tools ⇒ env still works: it is read from CUE,
  emitted into the manifest, cached, and injected identically. No binary is
  built, but the env path is independent of the binary path.

## Verification Architecture

### Purity boundary map

**Pure core (unit + property tested, no I/O):**
- `forest-manifest`: parse/validate the `env` field; env-name + value
  validation; `derive_shape` unaffected.
- `resolve_injection(component, local, ambient_keys)` — the merge/precedence
  function. This is the single most important pure unit; the ambient-wins
  invariant lives here.
- `user_config::parse` / `render_user_config` for `Dependency.env`
  (parse↔render round-trip).
- CUE-doc → manifest `env` extraction given an in-memory doc (pure transform).
- env-name/value validation shared helper.

**Effectful shell (integration tested):**
- Reading `std::env` at launch (to build `ambient_keys`).
- Reading/writing the env cache file under `~/.cache/forest/components/env/…`.
- Manifest fetch over gRPC.
- `Command::env` + `exec`.

The injection decision is fully determined by three plain inputs, so the
correctness-critical logic is verifiable without mocking the filesystem,
network, or process table.

### Provable / property-tested properties

- **P1 (ambient-wins):** ∀ key k ∈ ambient_keys, `resolve_injection(...)` does
  not contain k. ⇒ exec never overwrites an inherited var.
- **P2 (precedence):** if k ∉ ambient and k ∈ local ⇒ result[k] == local[k];
  else if k ∉ ambient and k ∈ component only ⇒ result[k] == component[k].
- **P3 (no-spurious-keys):** every key in the result is in component ∪ local and
  not in ambient.
- **P4 (idempotence/determinism):** result is a pure function of inputs; stable
  ordering for rendering and for the cached JSON (BTreeMap).
- **P5 (parse totality):** `forest-manifest::parse` and `user_config::parse`
  never panic on arbitrary input; invalid env ⇒ typed error.
- **P6 (round-trip):** `parse(render(cfg)) == cfg` including `Dependency.env`;
  `manifest env` survives publish→fetch unchanged.

Suggested tooling: `proptest` for P1–P4 over random (component, local, ambient)
maps; existing `#[test]` table tests for parse paths (mirroring the dense unit
tests already in `forest-manifest/src/lib.rs` and `user_config.rs`).

### Acceptance / integration checks

- **S1 (server round-trip):** publish a component with `env`, fetch its manifest,
  assert `env` is byte-for-byte preserved (verifies the registry stores
  `manifest_json` verbatim and no schema strips the field).
- **A1:** cold `forest global run` of an env-bearing tool writes the env cache
  file and the child sees the injected vars; a var exported in the parent shell
  is passed through unchanged (E1).
- **A2:** second run with the network unavailable (warm path) still injects the
  env from cache (B4 offline).
- **A3:** a per-tool `env` override in `forest.cue` wins over the component
  default but loses to an ambient export (B7).
- **A4:** `forest components show` displays the declared env (B8).
- **A5:** a pre-feature cache (no env file) runs fine and injects nothing (E3).
- **A6 (R4):** a per-tool `env` override hand-written in `forest.cue` survives a
  subsequent `forest global add <other-tool>` (render round-trips it).
- **A7 (R1):** `forest global update` moving a tool to a new version refreshes
  the cached `include` env to the new version's values.

## Implementation Map (for Phase 2 — not yet authorised)

1. **CUE schema** — add `#ForestInclude` and `include?: #ForestInclude` to
   `#ForestComponent` in `components/forest/sdk/spec.cue` and
   `cue/forest-sdk/spec.cue`.
2. **Publish** — read `forest.component.include` from the doc and add top-level
   `include` to the manifest JSON in all installable paths
   (`crates/forest/src/cli/components/publish.rs`: binary, `publish_prebuilt`,
   deno, `publish_external`); add the secret-name advisory (E8).
3. **forest-manifest** — add `include: Include` field + `Include` struct +
   parsing + `InvalidEnvName` / `InvalidEnvValue` errors + tests
   (`crates/forest-manifest/src/lib.rs`).
4. **Include cache** — new paths helper in `crates/forest/src/global/paths.rs`
   (`tool_include_dir(org, name, version)` → `env.json`); write on cold path and
   load (cold + warm) in `crates/forest/src/global/service.rs`.
5. **User config** — `Dependency.env` parse + render + validation
   (`crates/forest/src/global/user_config.rs`, `render_user_config` in
   `service.rs`) and the `user_config.cue` schema mirror.
6. **Resolution** — pure `resolve_injection` (new small module, e.g.
   `crates/forest/src/global/env.rs`) + property tests.
7. **Run** — wire injection into `RunCommand::execute`
   (`crates/forest/src/cli/global.rs`); have `resolve_to_cached_path` also return
   (or expose a sibling method to load) the component env so run can merge it.
8. **Visibility** — render `env` in `forest components show`
   (`crates/forest/src/cli/components/show.rs`) and the registry component page.
9. **Docs / example** — document the block; set a real default in
   `fungus/forest.cue` and bump its version.

## Open Questions (resolve before Phase 2)

- **Q1 — Declaration nesting. [RESOLVED]** Use a component-level `include { … }`
  container with `env` as its first member (forward-compat for future `files`
  etc.). Component-level (not under `upload`) so it applies to `external` tools
  too.
- **Q2 — Include cache key. [RESOLVED]** Key by **(org, name, version)** under
  `components/include/<org>/<name>/<version>/env.json` rather than literally
  beside the `<sha>` binary file — avoids the sha-dedup collision (E7) and loads
  trivially on the warm path (which knows org/name/version but only learns the
  sha via the lockfile). Honours the "shipped beside the binary in cache" intent
  while staying correct.
- **Q3 — Component page data source. [RESOLVED]** The registry stores and returns
  `manifest_json` **verbatim** (forage-server reads it via gRPC
  `GetComponentDetail`), so no proto/projection change is required for the data
  to reach clients. Structured display is opt-in additive:
  - CLI `forest components show`: JSON mode shows `include` automatically (raw
    `manifest_json`); **text mode needs an explicit render line** (`show.rs`).
  - Forage web page (`apps/forage`): raw-JSON tab shows it automatically;
    structured UI requires adding `include` to `manifest_view.rs::ManifestView`
    and the `component_detail.html.jinja` template.
- **Q4 — Secret-name advisory. [RESOLVED]** Yes — warn (don't block) at publish
  when a key matches `*_SECRET|*_TOKEN|*_KEY|PASSWORD|*_PASS` (case-insensitive).
- **Q5 — Validation strictness. [RESOLVED]** Env name `^[A-Za-z_][A-Za-z0-9_]*$`;
  values forbid only NUL. Matches the conventional env constraint and what
  `Command::env` accepts on Unix. **CUE map-key constraint syntax:**
  `{[=~"^[A-Za-z_][A-Za-z0-9_]*$"]: string}` (no `Name=` alias needed).

## Risks / Adversarial Findings (Phase-3 pre-mortem)

- **R1 — Same-version republish ⇒ stale env cache (highest-value catch).
  [RESOLVED by TASKS/024.]** The `(org, name, version)`-keyed include cache is
  only correct if a published version's content can't change. **TASKS/024**
  enforces exactly that for **stable** versions (content write-once; bump the
  version to change `include.env`), so the cached env can never be stale for a
  stable version. **Prerelease** versions remain mutable (TASKS/024 B4), so their
  cached env *can* be stale after an overwrite — acceptable, since prereleases
  are for iteration; refreshed by `forest global update` or a cache clear.
  We deliberately do **not** add a per-run manifest re-fetch (it would defeat the
  offline warm path). *Acceptance A7 covers `update` refreshing env.*
- **R2 — `Manifest` struct gains a field ⇒ every literal constructor breaks.**
  `forest-manifest::Manifest` is shared client+server. Adding `include` breaks
  any `Manifest { … }` struct-literal site. Audit + fix all construction sites
  (most go through `parse`, but check). New error variants are additive (safe).
- **R3 — Validation regex drift across three layers.** The env-name rule lives in
  CUE, forest-manifest, and user_config. Single source of truth on the Rust
  side: forest-manifest exposes `validate_env_name` / `validate_env_value`;
  user_config imports them. The CUE regex is documented to mirror it (and is
  re-enforced defensively at parse, so a hand-edited CUE that slips through is
  still caught).
- **R4 — `render_user_config` must round-trip `Dependency.env`.** If the CUE
  renderer drops `env`, an unrelated `forest global add/pin/remove` would
  silently wipe a developer's local override. *Acceptance A6: pre-existing env
  override survives a subsequent `forest global add`.* P6 round-trip property
  covers the unit level.
- **R5 — Empty-valued ambient var counts as "present".** `std::env::vars()`
  yields vars set to `""`, so an explicitly-empty ambient var wins over a
  component/local default (we skip it). Intended (ambient always wins); call it
  out so it isn't mistaken for a bug. (Refines E1/E4.)
- **R6 — `include` on a pure component (no tool facet) is inert.** Only tools run
  via `forest global run` consume `include.env`; a pure COMPONENT shape may carry
  it in its manifest but nothing injects it. Documented as reserved, not a bug.
- **R7 — Unified injection point at publish.** To avoid 4-way drift, read
  `forest.component.include` from the exported doc and attach it to the manifest
  in the **shared** manifest-assembly block (after the kind/descriptor branch),
  not separately per path. `include` is a regular CUE field (unlike the `#Tool`
  definition), so it appears in `cue export` output directly — no dedicated
  `cue eval -e` needed.
- **R8 — No way to preview resolved env.** Neither `run` (it execs) nor `which`
  shows what env would be injected. Optional affordance: `forest global which`
  could print the resolved `include.env` (post-precedence). Nice-to-have, not
  required for MVP — listed so it isn't forgotten.
