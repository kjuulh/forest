# 018: Global Tools — Org-Private Package Manager (VSDD Phase 1 Spec)

Status: **LOCKED**. Spec — not implemented. Architect decisions Q1–Q9 resolved (§4 + §5). Adversary review completed; trivial findings folded in. Ready for Phase 2 (failing tests first).
Method: Verified Spec-Driven Development. This file is the airtight contract that must survive the Adversary before any test or code is written.

---

## 0. Intent

Forest already ships components v2: binary plugins discoverable via the registry, content-addressable cache, sha256 lockfile, gRPC upload/download. This spec extends that model with a **tool facet**: a component can declare that it is *also* a plain CLI tool, and Forest can lazily install + expose it on the user's `PATH` via a `mise`-style shim directory.

**Framing — "enterprise mise":** the end-state of this work, including the deferred follow-ups, is an org-scoped equivalent of `mise` (or `asdf` / `nix-env`):

- **Local tools** — per-project dev tooling — live in `forest.cue` next to the project's existing component deps. The machinery is identical to today's components-v2; adding a tool facet to those is purely additive (deferred to a follow-up spec; see §0 out-of-scope).
- **Global tools** — per-user, cross-project tooling — live in `~/.config/forest/forest.cue` and are exposed via a PATH-prepended shim dir (this spec).
- **Distribution** — both flavours pull from the same registry, share the same content-addressable cache, share the same `NamespaceSource` resolver, and share the same publish pipeline (`UploadBinary` + `PublishManifest`, or the brew-style external-manifest variant in §1a.2b).

The differentiator from `mise` is org-scope and authorisation: tools are visible only to members of the publishing org, lockfiles bind to specific platform sha256s, and (in a future spec) signing binds the binary to an org-trusted keychain. The differentiator from a generic package manager is that tools are not a separate concept from components — they are components with one more optional facet. Local vs global is a question of **where the dependency record lives**, not what kind of artefact it is.

This spec lands the global half. The local half follows the same code paths once the OOS line in §0 moves.

User narrative:

> A user runs `forest global add cuteorg/scaffolder` once. They then run `eval "$(forest eval zsh)"` from `.zshrc`. Anywhere in their shell, typing `scaffolder foo bar` (or `forest global run cuteorg/scaffolder@0.1.0 -- foo bar`) lazily downloads the right binary from the org-private registry (if not cached), verifies its sha256 + signature, and execs it with the user's `argv`.

Org-level subscription: the user can also `forest global add cuteorg` (no `/name`) and receive the entire catalogue of tools that org publishes. They can ban individual tools out of that catalogue. Formalised in §1a.2c.

Out of scope for this spec (deferred to later VSDD passes):

- Project-local tool dependencies in the project's `forest.cue` (this spec covers only the **user-global** `~/.config/forest/forest.cue` surface). The mechanism is identical — a component carrying a tool facet, resolved via the shared `NamespaceSource` — so the follow-up spec is small: extend `ProjectComponents` to populate the per-project shim dir on `forest run`-entry, and emit project-scoped shims under e.g. `.forest/shims/` instead of `~/.cache/forest/global/shims/`. This is the "local half" of the enterprise-mise framing in §0.
- Windows. Shims are POSIX `sh`, `exec` uses Unix process semantics. Future spec can layer PowerShell shims.
- Fish / pwsh / nushell shells (only `zsh` + `bash` here).
- Tool *autocomplete* generation (separate concern; tools that ship their own completions are fine, but Forest does not interpose).
- Cross-platform binary fan-out / build pool (acknowledged ecosystem gap; tools just declare which `os/arch` pairs they ship and Forest refuses unsupported ones).
- **Signing / publisher keychains** — deferred to a follow-up spec. The eventual design will likely orbit a cosign-equivalent org-level keychain (so an organisation, not an individual, owns the trust root). This spec restricts trust to "sha256 of the artifact matches the manifest, transport is `https://`". §1a.3 documents the intended-but-deferred shape; `forest auth keygen`, `forest organisation trust-key`, `forest components publish --sign`, and `require_signed_tools` are NOT in this spec. Properties P8 and edge cases E5/E16/E31 are dropped accordingly.

---

## 1a. Behavioral Specification

### 1a.1 Tool facet in the component manifest

A component's `forest.component.cue` may declare a `#Tool` block alongside `#Commands` / `#Hooks`, conforming to the new `sdk.#ForestTool` schema:

```cue
import sdk "forest.sh/forest/sdk@v0"

#Tool: sdk.#ForestTool & {
    // The CLI name that will be exposed as a shim on PATH.
    // Must match /^[a-zA-Z][a-zA-Z0-9._-]{0,63}$/.
    name: string

    // If true, Forest invokes the binary with the user's argv directly
    // (`./binary arg1 arg2 ...`). If false, Forest invokes the binary using
    // the component protocol (`./binary _meta/describe`) — this spec
    // restricts in-scope tools to argv_passthrough=true. False is reserved.
    argv_passthrough: bool | *true

    // Optional one-line description rendered by `forest global list`.
    description?: string
}
```

The `_meta/describe` response (defined in `forest-sdk`, `ComponentDescriptor`) gains an optional sibling field `tool` of type:

```jsonc
{
  "name": "scaffolder",
  "argv_passthrough": true,
  "description": "Org scaffolder"
}
```

If `tool` is absent the component is a pure component (existing behavior, untouched). If `tool` is present *and* `argv_passthrough=true` the component is a **tool component**. A component may have both `methods` and `tool` (hybrid: invoked as tool from PATH, invoked as component from `forest run`).

The published manifest JSON (the blob stored via `PublishManifest`, served by `GetComponentManifest`) gains a top-level `tool` field mirroring the describe response, so that `forest global add` can detect tool-ness *without* downloading the binary.

**Upload pipeline reuse.** Tool binaries go through the **existing** `UploadBinary` / `PublishManifest` flow. There is no separate "tool upload" surface; the only change is that the manifest carries an optional `tool` field and an optional `kind: "external"` flag (§1a.2b). This means:

- Components written in **Rust** (via `forest-sdk`), **Deno** (via the JS SDK), or **Go** (via a future Go SDK) all use the same upload path — `forest components build && forest components publish`. The publisher's language is irrelevant; the registry sees content-addressed bytes + a JSON manifest. The tool facet is added by the CUE spec, not by the language toolchain.
- Pure tools (binary that only does argv-passthrough, no `_meta/describe` capabilities beyond the `tool` field) reuse the same pipeline. The SDK provides a thin helper for emitting a tool-only describe response so authors don't have to handwrite JSON.
- External (brew-style) tools (§1a.2b) skip `UploadBinary` because there's nothing to upload — the bytes live at an upstream URL — but reuse `PublishManifest` so registry, query, and authorisation paths are identical.

The net effect: one binary upload surface, one manifest surface, three flavours of "what's inside" (pure component / hybrid component / external tool). No new server-side storage tier, no language-specific code paths.

### 1a.2 Server / registry contract

A new gRPC method `ListOrgTools` is added on `RegistryService` (see §1a.2c). All manifest validation moves to `publish_manifest` (NOT `commit_upload`, which is the v1 file-upload path that external tool manifests do not traverse). `GetComponentManifest` returns the manifest JSON verbatim.

`publish_manifest` validates the following before persisting the manifest row:

1. `manifest.kind ∈ {"binary", "external"}` — exactly one.
2. If `manifest.kind == "external"`: `manifest.tool` MUST be present. (External manifests with no tool facet have no invocation protocol available — they cannot be run as components since there is no describe — so they are rejected at publish time with `INVALID_ARGUMENT: external manifest must declare a tool facet`.)
3. If `manifest.tool` is present:
   - `tool.name` matches `/^[a-zA-Z][a-zA-Z0-9._-]{0,63}$/`. `..` is rejected as a literal substring.
   - `tool.argv_passthrough ∈ {true}` (false is reserved, see §1a.1).
4. For each platform entry under `manifest.platforms[<os>_<arch>]`:
   - `sha256` is 64 hex chars, lowercase.
   - `os ∈ {"linux", "darwin"}` and `arch ∈ {"amd64", "arm64"}` (the supported matrix; widen in a follow-up spec).
   - `archive ∈ {"none", "tar.gz", "tar.xz", "tar.zst", "zip"}`.
   - If `archive ≠ "none"`: `binary_in_archive` is present and matches the path-canonicalisation rules in §1a.2d.
   - If `manifest.kind == "external"`:
     - `url` is present, starts with `https://`, parses as a valid URL, host is non-empty.
     - `archive_sha256`, if present, is 64 hex chars lowercase.
   - If `manifest.kind == "binary"`: `url`, `archive`, `binary_in_archive`, `archive_sha256` are all absent or null.
5. Manifest JSON length ≤ 64 KiB (DoS guard).
6. Per-(organisation, name, version) uniqueness — re-publish is `ALREADY_EXISTS`.
7. **Invocability:** if `manifest.kind == "binary"`, at least one of `manifest.methods` (non-empty) OR `manifest.tool` MUST be present. A binary that declares neither has no invocation surface (nothing for `forest run` or a shim to call) and is rejected. This rule, combined with rule 2 (`external ⇒ tool facet`), is what makes the shape taxonomy in §1a.2e total — every accepted manifest maps to exactly one of the four shapes.

Any rule failure → `INVALID_ARGUMENT` with a specific `field`+`reason` message.

Authorization is unchanged: tool components live in the same org-scoped namespace as components. A user can only `forest global add` a tool from an org they are a member of; resolution at runtime uses the same JWT/app-token chain.

### 1a.2d Path canonicalisation rules

Applied to `binary_in_archive` (both server-side at publish and client-side at extract):

1. Reject if length = 0 or length > 256.
2. Reject if any byte is NUL (`0x00`), CR (`0x0D`), LF (`0x0A`), or backslash (`0x5C`).
3. Reject if starts with `/` (absolute) or `~` (home-expansion).
4. Split on `/`. Reject any segment that is `..`, `.`, or empty (catches `a//b`, leading `/`, trailing `/`).
5. Reject if any segment matches `^\.` (hidden files — defence against `.git/`).
6. Normalise to NFC (unicode). The canonical form is what the manifest claims; any non-NFC input is rejected (`INVALID_ARGUMENT`).
7. Comparison against archive entries is byte-for-byte after each side has been NFC-normalised; mismatch → `binary not found`.

This is the canonical algorithm referenced by P12 (§1b.2) and E25.

### 1a.2b Lightweight external manifests ("brew-style")

Not every tool an org wants to expose has been (or needs to be) rebuilt and uploaded to the Forest binary cache. Sometimes the canonical upstream artifact already exists at a stable URL (HashiCorp releases, GitHub Releases tarballs, vendor mirrors). The user must be able to publish a **lightweight tool manifest** that points at external URLs + sha256 + extraction rules, with no binary upload.

A new manifest *origin* is introduced. The published manifest JSON (§1a.1) is extended:

```jsonc
{
  "kind": "binary",        // existing — binary was uploaded via UploadBinary
  // OR:
  "kind": "external",      // new — points at upstream URLs
  "tool": { "name": "...", "argv_passthrough": true, "description": "..." },
  "platforms": {
    "linux_amd64": {
      "sha256": "<hex>",
      "size": 12345678,
      // external-only fields below:
      "url": "https://releases.hashicorp.com/terraform/1.7.4/terraform_1.7.4_linux_amd64.zip",
      "archive": "zip",                  // ∈ { "none", "tar.gz", "tar.xz", "tar.zst", "zip" }
      "binary_in_archive": "terraform",  // path within archive (only if archive ≠ "none")
      "executable_mode": "0755"          // applied after extraction (only if archive ≠ "none")
    }
  }
  // signature: reserved for a future signing spec; not emitted in this spec.
}
```

For `kind = "binary"`: `url`, `archive`, `binary_in_archive` are absent; the binary is served by the registry via `DownloadBinary` (existing behavior).

For `kind = "external"`: at most one of `url` / `DownloadBinary` is used — for external manifests, the client fetches the URL directly, never the registry. The registry stores zero bytes of binary content for external tools.

**CLI surface — declaring external manifests:**

External tool manifests are **authored as ordinary Forest projects**, exactly like in-registry components. A project directory contains:

- `forest.cue` — declares project metadata + a `forest.component.external` block (instead of `forest.component.upload`/`forest.component.codegen`). This block holds the platform URLs, sha256s, archive layout.
- `forest.component.cue` — declares the `#Tool` facet (and nothing else; externals have no `#Commands`/`#Hooks`/`#Spec`).
- `cue.mod/module.cue` — same boilerplate as any other CUE project.

Worked example (`forest-ripgrep/` in [`examples/global-tools/`](../examples/global-tools/README.md)):

```cue
// forest-ripgrep/forest.cue
package forest_ripgrep

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
    name:         "ripgrep"
    organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
    name:    project.name
    version: "14.1.1"

    // No `codegen`, no `upload`. The `external` block makes this a
    // kind=external manifest at publish time.
    external: sdk.#ForestExternal & {
        platforms: [
            {
                os:                "linux"
                arch:              "amd64"
                url:               "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz"
                archive:           "tar.gz"
                binary_in_archive: "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg"
                sha256:            "ad3a44e3..."   // sha of extracted binary
                archive_sha256:    "4cf9f274..."   // optional: sha of the tar.gz
            },
            { os: "macos", arch: "arm64", ... },
        ]
    }
}
```

```cue
// forest-ripgrep/forest.component.cue
package forest_ripgrep
import sdk "forest.sh/forest/sdk@v0"

#Tool: sdk.#ForestTool & {
    name:             "rg"          // shim name on PATH (≠ project name on purpose)
    argv_passthrough: true
    description:      "Fast recursive grep, by BurntSushi"
}
```

**Single publishing command.** `forest components publish` is the only publishing surface for any artefact shape. It reads `forest.cue` and dispatches on which block is present:

- `forest.component.upload` present → build (existing pipeline) → `UploadBinary` → `PublishManifest` with `kind: "binary"`.
- `forest.component.external` present (and no `upload`) → no build, no upload → `PublishManifest` with `kind: "external"`.
- Both present, or neither → publish-time error.

This keeps "how do I ship a thing to the Forest registry?" a single mental model. There is no `forest tool publish` subcommand.

**Schema additions** to `forest-sdk`:

- `sdk.#ForestExternal` (new) — `{ platforms: [...sdk.#ForestExternalPlatform] }`.
- `sdk.#ForestExternalPlatform` (new) — `{ os, arch, url, archive, binary_in_archive?, sha256, archive_sha256? }` with the same invariants the server enforces (§1a.2): https-only URL, archive enum, `archive ≠ "none" ⇒ binary_in_archive`.
- `sdk.#ForestComponent` gains an optional `external?: #ForestExternal` field as a sibling of `upload?` and `codegen?`.
- `sdk.#ForestTool` (new — also referenced in §1a.1) — `{ name, argv_passthrough: bool | *true, description?: string }`.

Server behavior for `kind = "external"`:

- `publish_manifest` enforces §1a.2's rule set (URL scheme, sha256 hex format, archive enum, canonicalised `binary_in_archive`).
- The server does NOT fetch or verify the URL itself — the contents are validated by the client at download time. This keeps the server stateless w.r.t. third-party uptime.
- (Signing behavior deferred — see §0 Out-of-Scope.)

Client behavior at `forest global run` time:

```
resolve binary path:
  manifest = registry.GetComponentManifest(org, name, version)
  match manifest.kind:
    "binary":   download via registry.DownloadBinary (existing path)
    "external": download via HTTPS GET(manifest.platforms[os_arch].url)
                if archive ≠ "none": extract to temp dir, locate binary_in_archive
                set executable mode (0755 default)
  verify sha256 of the final executable file == manifest.platforms[os_arch].sha256
  move into cache at ~/.cache/forest/components/bin/<sha256>
  update lockfile
  exec
```

**Why the sha256 in the manifest applies to the final executable, not the archive:** if the upstream changes archive format (zip → tar.gz) but ships the same binary bytes, our content-addressed cache stays warm. The lockfile sha256 is the bytes the user actually `execv`s. We do verify the *archive* download integrity too — but via an additional `archive_sha256` field, OPTIONAL on the manifest; if present, the client also verifies it. If absent, only the extracted-binary sha is checked.

```jsonc
"platforms": {
  "linux_amd64": {
    "sha256": "<hex of extracted executable>",
    "archive_sha256": "<optional hex of downloaded archive>",
    ...
  }
}
```

Helper subcommand: `forest tool hash <url> [--archive zip] [--binary-in-archive scaffolder]` — downloads, computes archive_sha256 and the extracted binary sha256, prints both for the user to paste into their TOML.

### 1a.2c Org-catalog subscription (subscribe-to-everything mode)

In addition to pinning individual tools, a user may subscribe to an org's entire tool catalogue. This is the "I trust cuteorg, give me everything they publish" workflow.

CLI:

```sh
forest global add cuteorg                      # subscribe to org catalogue
forest global add cuteorg --ban badtool        # subscribe with ban list
forest global ban cuteorg badtool              # add to ban list later
forest global unban cuteorg badtool            # remove from ban list
forest global add cuteorg --pin myscaffolder=0.1.0  # subscribe but pin one tool
```

A subscription is distinct from a per-tool pin. The user-global config is `~/.config/forest/forest.cue` (see §1a.4 for migration from the legacy `forest.toml`). It represents the subscription as:

```cue
// ~/.config/forest/forest.cue
package forest

import sdk "forest.sh/forest/sdk@v0"

config: sdk.#UserConfig & {
    user: {}  // existing user kv table

    dependencies: {
        // Per-tool pins, equivalent to the old [dependencies] table.
        "cuteorg/scaffolder": { version: "0.1.0" }
    }

    org_catalog: {
        cuteorg: {
            enabled: true
            banned:  ["badtool", "experimental-thing"]
            // Optional per-tool pins inside a catalogue subscription.
            // Unpinned tools track the latest version on each `forest global update`.
            pins: {
                myscaffolder: "0.1.0"
            }
            // Optional alias map: their_name → my_name.
            aliases: {}
        }
    }
}
```

The `#UserConfig` schema lives at `cue/forest-sdk/user_config.cue` and uses identifiers that are valid in both CUE and Rust (`org_catalog` rather than `org-catalog` so the field maps cleanly through the existing serde-driven decoder). Forest still serialises the file deterministically (stable key order) so the `merge` operations are textually predictable.

Server contract: a new gRPC `ListOrgTools(organisation) -> stream OrgToolEntry` on `RegistryService`:

```proto
message OrgToolEntry {
  string organisation = 1;
  string name         = 2;       // component name
  string latest_version = 3;     // resolved server-side, see semantics below
  ToolFacet tool      = 4;       // name, argv_passthrough, description
  // Whether this component has *any* tool facet — used by the client to skip non-tool components.
  bool has_tool       = 5;
}
```

The server returns only components in `organisation` whose latest manifest declares a `tool` facet. The caller must have `OrgRole::Member` on the org. Pagination: the stream is server-driven; the client buffers but accepts back-pressure.

**`latest_version` semantics:** highest semver version EXCLUDING pre-releases (any version whose semver `pre` field is non-empty, e.g. `1.0.0-alpha.1`, `2.0.0-rc.2`). A component whose only published versions are pre-releases is NOT returned by `ListOrgTools`. To install a pre-release explicitly, the user must pin: `forest global add cuteorg/tool@1.0.0-alpha.1`. Pre-release-aware catalogue subscriptions are a follow-up spec.

**Yanked versions:** treated as not-published. A version that was published and then yanked is excluded from `latest_version`; the immediately-prior non-yanked version is chosen.

Client behavior on `forest global add <org>` (no `/name`):

1. Call `ListOrgTools(org)`.
2. For each entry not in `banned`, resolve to the highest version that the server declares (subject to local pins inside `[org-catalog.<org>.pins]`).
3. Install — same lazy semantics as individual `add`: do not download binaries eagerly. Just write the catalogue subscription into `forest.cue` and **create shims for each catalogue entry**. The shim's `forest global run` path lazily fetches on first invocation.
4. The lockfile is *not* pre-populated; lock rows are written on first run of each tool.

Updating: `forest global update` re-runs `ListOrgTools` and synchronises shims. Newly published tools in the catalogue get new shims. Removed-from-catalogue tools get their shims deleted (and their lock rows kept in case of rollback).

Banning: `forest global ban cuteorg badtool` writes `badtool` into `banned`, deletes the `badtool` shim, leaves the cached binary alone (cache GC is separate).

Conflict rules:

- If the user has both an explicit per-tool pin under `config.dependencies` AND a catalogue subscription that lists the same component, the explicit per-tool pin wins for resolution and the catalogue entry is treated as redundant. `forest global list` flags this.
- Two catalogue subscriptions whose tools collide on `shim_name` are an error at `forest global sync` time: the second add fails with the same collision message as §1a.8, suggesting `--as` per-tool aliasing for the catalogue entry that should yield.

**Canonical evaluation order for catalogue entries (single source of truth — referenced by P15):**

For each catalogue subscription `config.org_catalog.<org>`:

```
let upstream_entries = ListOrgTools(org)              // server-side
let banned_set       = config.org_catalog.<org>.banned     // set of upstream tool.name
let aliases          = config.org_catalog.<org>.aliases    // map<upstream_tool.name, shim_name>
let pins             = config.org_catalog.<org>.pins       // map<upstream_tool.name, version>

for entry in upstream_entries:
    if entry.tool.name ∈ banned_set: skip
    let resolved_version = pins.get(entry.tool.name) or entry.latest_version
    let shim_name        = aliases.get(entry.tool.name) or entry.tool.name
    emit (org, entry.name, resolved_version, shim_name)
```

Bans, aliases, and pins are ALL keyed by the **upstream `tool.name`** as returned by the server. The alias affects only the shim filename on disk, not lookup keys in user config and not the `tool.name` embedded in the shim body. E36 ("alias for a tool that doesn't exist in the catalogue") is therefore a no-op (the upstream key is never matched). Same for E34 (ban) and the pin equivalent.

This means a single `forest global add cuteorg` plus a single `eval "$(forest eval zsh)"` in `.zshrc` give a user the entire org's toolbox on `PATH`, lazily.

### 1a.2e Registry shape taxonomy

The registry now stores four distinguishable kinds of artefact. Search, listing, and detail RPCs MUST carry a shape discriminator so callers (the web UI in a separate repo, `forest components search`, future `forest tool list-org`, IDE integrations) can render them differently without re-inspecting the manifest blob.

**The four shapes** — derived deterministically from `(manifest.kind, manifest.tool, manifest.methods)`:

| Shape | `manifest.kind` | `manifest.tool` | `manifest.methods` non-empty | Invocation surface | Hosted where |
|---|---|---|---|---|---|
| `COMPONENT` | `binary` | absent | yes | component protocol only (`forest run <command>`, hooks) | Forest binary cache |
| `HYBRID_COMPONENT` | `binary` | present | yes | component protocol AND argv passthrough via shim | Forest binary cache |
| `TOOL_BINARY` | `binary` | present | no (only `_meta/describe`) | argv passthrough only | Forest binary cache |
| `TOOL_EXTERNAL` | `external` | present | n/a (no describe) | argv passthrough only | Upstream URL |

Two combinations are **invalid** and rejected at `publish_manifest`:

- `binary` + no `tool` + no `methods` → `INVALID_ARGUMENT: component must declare either methods or a tool facet (nothing to invoke)`. Added as validation rule 7 in §1a.2.
- `external` + `methods` declared → `INVALID_ARGUMENT: external manifests cannot declare methods (no describe protocol available)`. Enforced by the `#ToolManifest` CUE schema (it has no `methods` field) and re-enforced at the gRPC layer in case a publisher hand-crafts JSON.

**Persistence:** the `components` table gains a `shape TEXT NOT NULL CHECK (shape IN ('component','hybrid_component','tool_binary','tool_external'))` column. The migration backfills existing rows as `'component'` (today's components have no tool facet). `publish_manifest` writes the computed shape transactionally with the manifest row.

**gRPC additions:**

```proto
enum ComponentShape {
  COMPONENT_SHAPE_UNSPECIFIED   = 0;
  COMPONENT_SHAPE_COMPONENT     = 1;  // methods-only
  COMPONENT_SHAPE_HYBRID        = 2;  // methods + tool
  COMPONENT_SHAPE_TOOL_BINARY   = 3;  // tool-only, hosted
  COMPONENT_SHAPE_TOOL_EXTERNAL = 4;  // tool-only, external URL
}

// Extend SearchComponentsResponse.components[*] with:
message ComponentSearchResult {
  // ... existing fields ...
  ComponentShape shape       = 10;
  ToolFacet      tool        = 11;  // populated for HYBRID / TOOL_*
  repeated string methods    = 12;  // names only, populated for COMPONENT / HYBRID
  string upstream_host       = 13;  // populated for TOOL_EXTERNAL (e.g. "releases.hashicorp.com" — host only, no path)
}

// Extend GetComponentDetailResponse with:
message ComponentDetail {
  // ... existing fields ...
  ComponentShape shape            = N;
  ToolFacet      tool             = N+1;
  repeated MethodInfo methods     = N+2;  // full method descriptors for COMPONENT / HYBRID
  string upstream_url             = N+3;  // full URL for TOOL_EXTERNAL — viewable, surfaced only on detail
  repeated PlatformInfo platforms = N+4;  // os, arch, size, archive type; sha shown on hover
}
```

**`ListOrgTools` filtering refinement.** Per §1a.2c it streams "components that have a tool facet". With the shape taxonomy this is now precise: `shape IN ('hybrid_component', 'tool_binary', 'tool_external')`. Pure `COMPONENT`s are excluded — they're not installable as shims. The stream's `OrgToolEntry` gains the same `shape` field so the catalogue subscription UX can show which entries are externals (different trust posture) vs in-registry binaries:

```proto
message OrgToolEntry {
  // ... existing fields ...
  ComponentShape shape       = 6;
  string upstream_host       = 7;  // populated for TOOL_EXTERNAL only
}
```

**`upstream_host` exposed, full URL gated.** For `TOOL_EXTERNAL` entries, search results show only the host (e.g. `releases.hashicorp.com`) so an org admin scanning the catalogue can immediately see "this tool is sourced from X" without expanding each entry. The full URL is only returned by `GetComponentDetail` (one row at a time, authenticated). Rationale: a directory listing of 200 external tools shouldn't double as a list of attack-surface URLs.

**CLI rendering** (this spec, just the CLI; the web UI lives in a separate repo and consumes the same gRPC):

- `forest components search <q>` — print one row per result with a shape badge: `[component]`, `[hybrid]`, `[tool]`, `[tool-ext]`. Externals additionally show `← releases.hashicorp.com`.
- `forest components show <org>/<name>` — render shape + relevant section: methods list for `COMPONENT`/`HYBRID`, tool facet + platforms + (for externals) full URL for `TOOL_*`.
- `forest global list` — only ever shows `HYBRID` / `TOOL_BINARY` / `TOOL_EXTERNAL` entries; per-row badge distinguishes `[hybrid]` / `[tool]` / `[ext]`. Pure `COMPONENT`s never appear here (E1 stands).

This taxonomy is the contract the web UI will consume. The UI does not re-derive shape from manifest fields; it trusts the server-computed `shape` enum. That keeps the deriving logic in exactly one place (server-side at publish time) and prevents UI/CLI drift.

### 1a.3 Signing (DEFERRED — recorded for the future spec)

Signing is deferred to a follow-up VSDD spec. The eventual model will likely be an **org-level keychain** (cosign-equivalent) rather than per-user Ed25519 keys: the organisation owns a set of trusted signing keys (managed via a `forest organisation keychain` surface), and any tool published under that org is signed against the org keychain. This is closer to how container registries and Sigstore work, and avoids the "every developer is a publisher" complexity.

Out of this spec's scope therefore:

- No `forest auth keygen`, `forest organisation trust-key`, or `forest components publish --sign`.
- No `signature` field in manifest JSON. The field is reserved at the schema level but never produced and never consumed.
- No `require_signed_tools` org policy.
- No `publisher_keys` / `org_trusted_keys` tables.

What this spec **does** rely on for tool integrity:

1. `https://`-only URLs for external manifests (rejecting `http`, `file`).
2. sha256 of the final executable bytes verified against the manifest, at download time and again before exec (§1b P3, P13).
3. Org-scoped authorization on `GetComponentManifest` / `DownloadBinary` / `ListOrgTools` (existing).

This is weaker than signature-based trust (a compromised registry could push a new manifest with attacker-controlled `url` + `sha256`), and that gap is the explicit motivation for the signing follow-up spec. The architecture deliberately leaves a clean seam (the `verify_signature` no-op stub in the resolver) so the follow-up can drop in without restructuring.

### 1a.4 User-global config and lock

**Config file (user-edited, CUE):** `~/.config/forest/forest.cue` (i.e. `$XDG_CONFIG_HOME/forest/forest.cue`). Holds `[dependencies]` (per-tool pins), `[org_catalog]` subscriptions, and the existing user kv section.

**`#UserConfig` schema:**

```cue
// cue/forest-sdk/user_config.cue
package sdk

#UserConfig: {
    // Open struct of arbitrary string→string user-set keys.
    // Preserves the existing `[user]` table semantics from forest.toml.
    user: [string]: string

    // Per-tool pins. Key is "<org>/<name>", value carries the resolved version.
    dependencies: [string]: {
        version: string
    }

    // Org-catalog subscriptions. Key is the organisation name.
    org_catalog: [string]: {
        enabled: bool | *true
        banned:  [...string] | *[]
        pins:    [string]: string  // upstream tool.name → version
        aliases: [string]: string  // upstream tool.name → local shim_name
    }
}
```

**No migration from `forest.toml`.** The user-global `forest.toml` is the human-readable patch config only — it has never held credentials (auth state lives separately under `~/.config/forest/credentials*` / native keychain via `crates/forest/src/cli/auth/`). It is therefore safe to **leave the legacy file untouched** and treat `forest.cue` as the new exclusive source of truth from this spec onward.

Concretely:

- Forest **reads only `forest.cue`** in code paths introduced by this spec. The existing `crates/forest/src/user_config.rs` (TOML reader) is replaced by a CUE reader; the TOML reader code is deleted.
- If a user has a pre-existing `forest.toml`, it is silently ignored. A one-time stderr notice on first run is fine but not required: `info: legacy forest.toml is no longer read; redeclare your dependencies in forest.cue`.
- No `.bak` rename, no merge, no race window. The whole migration paragraph and edge cases tied to it are gone.

Authentication state (tokens, refresh tokens, organisation context) continues to live in its existing location and is **out of scope for this spec**.

**Lockfile (derived, not user-edited):** `$XDG_STATE_HOME/forest/forest.lock` (default `~/.local/state/forest/forest.lock`). Putting it under `state` rather than `config` reflects that it is regenerable from registry + config; putting it under `state` rather than `cache` reflects that it is security-critical pinning data and must NOT be wiped by a "clear cache" operation.

Same line-format as the per-project `forest.lock`:

```
# forest.lock — do not edit manually
cuteorg/scaffolder@0.1.0 linux/amd64 sha256:abc...
```

Path entries are not allowed in the global lockfile — the loader is a **strict-mode parser** that returns `Err(LockError::PathEntryNotAllowed)` if a `path:` line is encountered (the existing project-lock parser accepts them; the global loader wraps it with this guard). The state directory is created with `0700` permissions; the lockfile itself with `0600`.

### 1a.5 New / changed CLI commands

| Command | Behavior |
|---|---|
| `forest global add <org>/<name>[@<ver>]` | Existing. Now, after writing `forest.cue`, creates a shim if the component has a `tool` facet. If `<ver>` is omitted, resolve latest from registry and pin the resolved exact version into `forest.cue`. Binary is NOT downloaded eagerly — the shim handles lazy download on first invocation (consistent with §1a.2c semantics). |
| `forest global add <org>` (no `/name`) | NEW. Subscribes to the org's tool catalogue per §1a.2c. Flags: `--ban <name>` (repeatable), `--pin <name>=<ver>` (repeatable), `--alias <their>=<mine>` (repeatable). Creates shims for every catalogue entry, lazily resolved. |
| `forest global ban <org> <tool>` | NEW. Add `tool` to `config.org_catalog.<org>.banned`. Deletes the shim. |
| `forest global unban <org> <tool>` | NEW. Remove `tool` from the ban list. Re-creates the shim. |
| `forest global pin <org>/<tool> <ver>` | NEW. Add or update a per-tool pin inside a catalogue subscription. |
| `forest global unpin <org>/<tool>` | NEW. Remove the pin; the tool tracks `latest` again on next `update`. |
| `forest global remove <org>/<name>` | NEW. Removes the per-tool dependency from `forest.cue`, deletes the shim, leaves the binary in cache (cache GC is separate). |
| `forest global remove <org>` (no `/name`) | NEW. Unsubscribes from an org catalogue (`config.org_catalog.<org>` is deleted). Deletes every shim that was emitted exclusively from this subscription. Per-tool dependencies under `config.dependencies` for the same org are NOT removed. |
| `forest global list` | NEW. Prints installed tools as a table: `name  org  version  status`. `status` ∈ `{cached, missing}`. |
| `forest global run <tool>[@<ver>] [-- args...]` | NEW. Resolves `<tool>` (bare name uses `forest.cue` pin; `name@ver` overrides). If the binary for that version is not in cache, downloads it. Verifies sha256. Exec's the binary with `args` as argv. The shim invokes this. |
| `forest global which <tool>` | NEW. Prints absolute path of the resolved binary (after fetching if missing). |
| `forest global update [<tool>]` | NEW. With no arg: re-resolves every pinned dep, fetches new versions, updates `forest.cue` + global lock + reissues shims. With an arg: same but scoped to one tool. |
| `forest global sync` | NEW. Ensures shim dir exactly matches `forest.cue`: creates missing shims, deletes orphaned ones. Run automatically by `add`/`remove`/`update`. |
| `forest global verify` | NEW. Walks the cache, re-hashes every `bin/<sha>` entry, deletes mismatches. Defence-in-depth for T1 (§1a.9b). Not on the warm path. |
| `forest eval zsh` / `forest eval bash` | NEW (top-level, not under `shell`). Prints a single block that's `eval`-able from the user's rc file. The block prepends `$HOME/.cache/forest/global/shims` to `PATH` (idempotently). Stays narrowly focused on PATH; other shell helpers continue to live under `forest shell zsh`. |
| `forest tool hash <url> [--archive zip] [--binary-in-archive scaffolder]` | NEW. Helper for authoring external manifests: downloads the URL, computes `archive_sha256` and the extracted-binary sha256, prints both for the author to paste into a project's `forest.cue`. Doesn't write to the registry. |

`forest tool` is introduced for dev helpers (currently just `hash`). External tools are **published via `forest components publish`** from inside the tool's project directory — same command as in-registry components, dispatched on whether `forest.cue` declares `upload:` (binary) or `external:` (external). There is no `forest tool publish`.

`forest shell zsh` continues to exist and continues to emit the `forest-tmp` helper; `forest eval zsh` is the new, install-path command and is separate by design (different responsibility, different output, different cadence).

### 1a.6 Shim format

A shim is a portable POSIX shell script:

```sh
#!/bin/sh
# forest shim — do not edit
exec forest global run cuteorg/scaffolder -- "$@"
```

File mode `0755`. One shim per shim-name. Location: `$HOME/.cache/forest/global/shims/<shim_name>`. Forest does not generate compiled-binary shims in this spec (defer to a later perf-driven spec).

**Single canonical rule for `tool.name` / `shim_name` / `--as`:**

Three concepts, one resolution order:

1. **`tool.name`** — declared in the component manifest (or external CUE). This is the upstream identity. It's what `_meta/describe` emits, what `ListOrgTools` returns, and what `[org_catalog.X.banned/aliases/pins]` keys against. Never overridden by the client.
2. **`shim_name`** — the name of the shim file on disk. Defaults to `tool.name`. Overridden by client-side aliasing in exactly two places:
   - `forest global add <org>/<name> --as <alias>` writes `--as <alias>` against the dep entry in `config.dependencies` (a new optional field `shim_name`).
   - Catalogue aliases: `config.org_catalog.<org>.aliases[<upstream>] = <local_shim_name>`.
3. **Shim body** — always embeds the qualified `<org>/<name>` (NEVER the shim_name or alias). The shim invokes `forest global run <org>/<name> -- "$@"`. This means the alias is purely a presentation concern; resolution always goes through the upstream identity.

Collision rule: if two resolved `shim_name` values are equal, `forest global add`/`sync` fails. Aliasing exists exactly to resolve such collisions.

**Atomic shim writes:** Every shim file is written via `tempfile + fsync + rename` into the shims directory. A concurrent shell never observes a half-written shim. Shim deletion (on `remove`/`unban`/`sync`) uses `unlink`; partial removal is benign because PATH lookup re-evaluates each invocation.

`forest global sync` enforces: every file in the shim dir corresponds to a current resolved entry; every resolved entry whose manifest has a `tool` facet has a shim. Orphan shims (file present, no resolved entry) are deleted only if the file's first 2 lines match the `# forest shim` marker — Forest never deletes shell scripts it didn't author.

### 1a.7 `forest eval zsh` output

Exact, byte-stable output for zsh:

```sh
# forest eval zsh — added to PATH idempotently
case ":$PATH:" in
  *":$HOME/.cache/forest/global/shims:"*) ;;
  *) export PATH="$HOME/.cache/forest/global/shims:$PATH" ;;
esac
```

The bash variant is byte-identical (POSIX `case` works in both shells). The string is deterministic; same input → byte-identical output.

### 1a.8 Tool name collision rules

Two tools from different orgs may register the same `tool.name`. The shim is named after `tool.name`, so `forest global add` MUST refuse to create a colliding shim. Resolution:

1. If `forest.cue` already has a dep whose component manifest declares `tool.name = X`, and the user runs `forest global add org2/foo` whose manifest also declares `tool.name = X`, the command fails with `error: tool name 'X' already provided by cuteorg/scaffolder@0.1.0 (alias the new tool with --as <name>, or remove the existing one)`.
2. `forest global add ... --as <alias>` overrides the shim name (but not the underlying `tool.name` in describe output).
3. Same `tool.name` from the same component at different versions is not a collision (only one version is pinned at a time).

### 1a.9 Lazy install + version resolution at `run`

Pseudocode for `forest global run <ref> -- args`:

```
parse <ref> →
  if "<org>/<name>": pin = required, version = lookup forest.cue
  if "<org>/<name>@<ver>": pin = explicit, version = <ver>
  if "<bare-name>": resolve via the shims directory (Q7.a):
                      readdir ~/.cache/forest/global/shims/
                      if exactly one shim file's name == bare-name:
                        read its body, extract <org>/<name>; version = pinned
                      if zero: error "tool '<bare-name>' is not installed"
                      if more than one: error "ambiguous tool name '<bare-name>'; \
                                              specify <org>/<name>[@<ver>]"
  else error

resolve binary path:
  expected_sha = lockfile.get(org, name, version, os, arch)
  cached_path = ~/.cache/forest/components/bin/<expected_sha>
  if expected_sha is None or cached_path missing:
    // COLD PATH (NFR ≤ 2 s for 20 MB)
    manifest = registry.GetComponentManifest(org, name, version)
    download_to_temp:
      match manifest.kind:
        "binary":   stream DownloadBinary into a temp file
        "external": HTTPS GET manifest.url; if archive ≠ "none", extract binary_in_archive
    verify sha256 of the temp file == manifest.platforms[os_arch].sha256
    // The cache key IS the sha — by writing to bin/<sha> we make the
    // content-address invariant true by construction. Concurrent writers
    // produce identical bytes; tempfile+rename makes the final move atomic.
    fsync temp file
    rename temp file → bin/<sha256>   // CAS-by-content-address
    update lockfile (only for the version actually executed)
  // WARM PATH (NFR ≤ 60 ms)
  // No re-hash: cached_path's filename is its sha by construction (see §1a.9b).
  exec cached_path with args
```

### 1a.9b Content-address trust on warm path (resolves the P3 ↔ ≤60 ms NFR collision)

The cache layout `~/.cache/forest/components/bin/<sha256>` makes the filename the cryptographic name of the bytes. Forest's only writer is its own `tempfile + rename` path (§1a.9), which performs sha verification BEFORE the rename. Therefore on the warm path, opening `bin/<sha>` and exec'ing it is correct by construction, with one explicit assumption:

> **Trust assumption (T1):** the `bin/` directory is writable only by the user. An attacker with write access to `~/.cache/forest/components/bin/` has already won (they can also rewrite shims, the `forest` binary, etc.). The spec does not defend against that threat model.

Concretely:

- Warm path does NOT re-hash the cached binary before `exec`. P3 is reinterpreted as: "the cache invariant `bin/<sha>.bytes hash to sha` is maintained by the writer path; the reader path trusts it." Property P3 is restated below (§1b.2) accordingly.
- A user can request a one-off integrity scrub: `forest global verify` (NEW, see §1a.5) iterates the cache and re-hashes every entry, deleting any whose filename ≠ content sha. This is the defence-in-depth knob and is NOT on the warm path.
- Lockfile mismatch (entry says `sha = X` but `bin/X` does not exist, or `bin/X` exists but lockfile says something else for the same org/name/version): treated as cache miss, fall through to cold path; the cold path re-verifies. Spec property P3 is satisfied because the only way to reach `exec` is either via the trusted warm path (T1) or via the verified cold path.
```

If the registry is unreachable AND the version is cached AND lockfile sha matches: succeed offline. If not cached AND offline: error `tool 'X' is not installed and registry is unreachable`.

### 1a.10 Edge case catalog (exhaustive)

| # | Scenario | Required behavior |
|---|---|---|
| E1 | `forest.cue` references a component that has no `tool` facet | `forest global add` succeeds (it's a regular component dep) but no shim is created; `forest global list` shows it under a `components` section or omits it (decision: **omit**, this command is the *tools* surface). |
| E2 | User runs `forest global run X` where X is not in `forest.cue` | Error: `tool 'X' is not installed; run 'forest global add <org>/<name>' first`. No registry call. |
| E3 | Cached binary's sha256 ≠ lockfile entry | Error: refuse to exec, prompt user to `forest global update`. Never silently re-download. |
| E4 | Registry returns a binary whose sha256 ≠ manifest's claimed sha256 | Error from registry-side hash check (already implemented in `upload_binary`); on the *download* path this spec adds a client-side check that the streamed bytes hash to the lockfile pin. |
| E5 | _(reserved — signing edge case, deferred with §1a.3)_ | n/a |
| E6 | Two concurrent `forest global add` invocations | File-lock on `~/.config/forest/.lock` for the duration of the read-modify-write of `forest.cue` + `forest.lock` (the lockfile lives in `$XDG_STATE_HOME/forest/`, see §1a.4). Stale lock detection: pid + timestamp; reaped after 60 s if pid is gone. |
| E7 | `~/.config/forest/forest.cue` missing or unreadable | `forest global list` succeeds with empty result; `forest global add` creates the file with `0600` mode. Legacy `forest.toml` is silently ignored (§1a.4). |
| E8 | Shim dir is on `$PATH` but does not exist on disk | `forest eval zsh` does not pre-create it (it would happen at `add` time). Shell PATH entry pointing at a missing dir is benign. |
| E9 | Tool name contains `/`, `\`, null byte, or starts with `-` | Rejected at manifest validation time (server) and again client-side defensively. |
| E10 | Component is published with both `commands` and a `tool` facet | Both work: `forest run <component-name>` still uses the protocol, shim still does argv-passthrough. They are independent invocation paths over the same binary. |
| E11 | User on `linux/arm64` but tool only published for `linux/amd64` | Error: `tool not available for linux/arm64; published for: linux/amd64, darwin/arm64 (+N more)`. Up to 4 platforms listed, then `+N more`. |
| E12 | Tool binary returns non-zero exit | `forest global run` exits with the same code (transparent passthrough). |
| E13 | Tool binary receives a signal (e.g., Ctrl-C) | `forest global run` does not trap; the binary is `exec`'d (not forked), so the signal goes to it directly. |
| E14 | User's home dir is read-only / shim dir cannot be created | Error at `forest global add` time with a clear message. |
| E15 | `forest global remove` for a name that isn't in `forest.cue` | No-op success with a stderr note. |
| E16 | _(reserved — signing edge case, deferred with §1a.3)_ | n/a |
| E17 | User downgrades a tool: `forest global add org/x@0.0.9` after `@1.0.0` is pinned | `forest.cue` is rewritten to `0.0.9`, lockfile is replaced for that org/name (only one row per org+name+platform), shim re-issued. |
| E18 | Bare-name resolution where two unrelated dep entries point at the same component (impossible: `dependencies` keys are unique) | Cannot occur by construction; assert in code. |
| E19 | `forest eval zsh` invoked twice in the same shell | PATH check ensures no duplicate entry (idempotent by the `case` statement). |
| E20 | Tool component renames its `tool.name` between versions | Treated as a different tool: shim for old name is orphaned (deleted on `sync`), new shim created. Document this in `--help` for `update`. |
| E21 | External manifest URL is `http://` (not `https`) | Rejected at `PublishManifest` time with `INVALID_ARGUMENT`; rejected client-side defensively too. |
| E22 | External manifest URL is reachable but returns a 4xx/5xx | Client error: `failed to download tool <name>: HTTP 404 from <url>`. No partial write to cache. |
| E23 | External manifest URL returns 200 but body sha256 (archive_sha256 if set, else extracted-binary sha256) ≠ expected | Refuse to cache; delete temp dir; error with both expected and actual. |
| E24 | External manifest declares `archive = "zip"` and the downloaded body is not a valid zip | Error during extraction phase, no cache write. Distinguish: corrupted vs wrong format. |
| E25 | `binary_in_archive` path traverses outside the extraction dir (e.g., `../../etc/passwd`) | Reject before extraction (path canonicalisation check). |
| E26 | `binary_in_archive` does not exist inside the extracted archive | Error: `binary 'X' not found in archive (top-level entries: a, b, c)`. |
| E27 | External URL serves different bytes on retry (CDN drift, time-bomb) | First successful verification wins; the cached file is content-addressed so subsequent runs hit the cache. A `forest global update` re-fetches and would catch the drift. |
| E28 | External manifest has `archive = "none"` but server returns a body that begins with a known archive magic (gzip, zip, tar) | Warn but proceed: the manifest's contract is authoritative. The sha256 verification still gates execution. |
| E29 | External manifest's `url` resolves to a private IP / 127.x.x.x | The DNS resolution itself is the user's machine's responsibility; Forest does not blocklist. (Document: don't ship `localhost` URLs in shared manifests.) |
| E30 | Mixing `kind = "binary"` and `kind = "external"` for different versions of the same component | Allowed: each version is an independent manifest. `forest global list` shows the origin per-installed version. |
| E31 | URL is changed in the registry after publish | Cannot happen: manifest JSON is immutable per version on the server (already enforced by `PublishManifest` semantics: one manifest per (org, name, version)). Re-publish at the same version is rejected with `ALREADY_EXISTS`. (This invariant is exactly what a future signing spec would *lock down cryptographically*; in this spec it is enforced only by server policy.) |
| E32 | User subscribes to an org catalogue, then a new tool is published whose `shim_name` collides with an existing per-tool dep | `forest global update` fails for that tool; surface the collision in the update report, offer `--alias` resolution. Other tools in the same `update` still apply. |
| E33 | User loses org membership between `forest global add cuteorg` and the next `update` | `ListOrgTools` returns `PERMISSION_DENIED`. Client preserves existing pins but emits a warning; shims stay until `forest global remove cuteorg` is run. (Decision: do not auto-delete — losing access shouldn't break a running shell session.) |
| E34 | `[org-catalog.X].banned` lists a tool that doesn't exist in the org's catalogue | No-op; not an error. Defends against typos becoming silent allowance later. |
| E35 | Both `[dependencies]` and `[org-catalog.<org>]` provide the same `shim_name` for the same component | Explicit per-tool pin wins; catalogue entry is shadowed. `forest global list` annotates "shadowed by [dependencies]". |
| E36 | Catalogue alias `[org-catalog.X.aliases] their_name = "my_name"` where `their_name` isn't in the catalogue | Treated like E34: no-op. |
| E37 | A tool is removed from an org's published catalogue (the publisher unpublishes the manifest) | `ListOrgTools` no longer returns it; `forest global sync` deletes the shim. The lockfile entry persists for rollback. `forest global update` reports the removal. |
| E38 | Two concurrent `forest global run` cold-miss the cache for the same `(org, name, version, os, arch)` | Both download, both verify sha, both `tempfile + rename` into `bin/<sha>`. Last writer wins; bytes are identical by sha invariant. No corruption. (See §1a.9.) |
| E39 | A cached `bin/<sha>` is corrupted on disk (bit rot, manual edit) | Warm path execs corrupted bytes (T1 assumption — Forest does not re-hash). `forest global verify` is the explicit recovery: it rehashes the cache and deletes mismatches; subsequent runs re-fetch. |
| E40 | macOS quarantine xattr | Forest's HTTPS download path writes to a tempfile via `std::fs::File::create` + `write_all`, which does NOT set `com.apple.quarantine` (only Foundation download APIs and Finder do). The resulting `bin/<sha>` is exec-clean on macOS without `xattr -d`. Verified by an acceptance test gated on `cfg(target_os = "macos")`. |
| E41 | `forest.cue` contains a value the `#UserConfig` schema rejects | Error at read time with the offending CUE path quoted. `forest global add` refuses to write; the user must fix the file by hand. (CUE evaluation produces the diagnostic.) |
| E42 | User runs `forest global run` with `<org>/<name>` not declared in `forest.cue` but reachable via an `org_catalog` subscription | Resolve via the catalogue evaluation in §1a.2c; if the resulting shim_name matches, succeed. No implicit promotion to `config.dependencies`. |
| E43 | Publisher attempts to publish a `binary` manifest with neither `methods` nor `tool` | Rejected at `publish_manifest` per validation rule 7 (§1a.2). Error: `INVALID_ARGUMENT: component must declare either methods or a tool facet (nothing to invoke)`. |
| E44 | Publisher attempts to publish an `external` manifest with `methods` declared | Rejected: CUE schema has no `methods` field on `#ToolManifest`. If the publisher bypasses CUE and hand-crafts JSON, the gRPC layer rejects with `INVALID_ARGUMENT: external manifests cannot declare methods`. |
| E45 | UI / CLI receives a manifest row with `shape == COMPONENT_SHAPE_UNSPECIFIED` from a pre-spec server | Treat as `COMPONENT` (the backfill default) — preserves existing semantics for clients connecting to an upgraded server before they're upgraded themselves. |

### 1a.11 Non-functional requirements

- **Startup latency**: shim → exec'd binary must add ≤ 60 ms on a warm cache (no registry call). Measured by `time scaffolder --version` minus the binary's intrinsic time. Achieved by making `forest global run` a thin path: parse, stat lockfile + cache, `execv`. No tokio runtime, no gRPC client init on the warm path.
- **Cold install**: a 20 MB binary over a registry on localhost must complete in ≤ 2 s end-to-end.
- **Disk**: shim dir ≤ 4 KB per tool. Cache reuse across tools means N tools at version V share zero storage with N tools at version V+1 (different sha → different file).
- **Memory**: the warm-path resolver (`forest global run`) must not allocate > 16 MB resident.
- **Security**: `~/.config/forest/forest.cue` is created `0600`; `$XDG_STATE_HOME/forest/forest.lock` is created `0600` inside a `0700` parent dir. All HTTPS fetches use `rustls` with system roots and no MITM override flags.

---

## 1b. Verification Architecture

### 1b.1 Purity boundary map

The system is decomposed into a **pure core** and an **effectful shell**. The boundary is drawn so model-checkers can reason about the core without mocking infrastructure.

**Pure core (deterministic, no I/O, no time, no random):**

- `forest::global::resolver` — given `(user_config: UserConfig, lockfile: LockFile, manifest: Manifest, ref: ToolRef, platform: (os, arch))` returns a `Plan` ∈ `{UseCached(path) | FetchFromRegistry{version, expected_sha} | FetchFromUrl{url, archive, binary_in_archive, archive_sha?, expected_sha} | Error(reason)}`. No syscalls. The plan variant `FetchFromUrl` is generated only when `manifest.kind == "external"`; `FetchFromRegistry` only when `kind == "binary"`. Mutual exclusivity is a typestate invariant of the function — proven by P11 below.
- `forest::global::lockfile` — already pure (parse/serialize). Extend with `merge`, `replace_for_name_version`, used by writer.
- `forest::global::shim` — `shim_script_for(tool_name, qualified_ref) -> String`. Pure function. Deterministic.
- `forest::global::names` — tool-name validation, alias resolution (`UserConfig` + alias map → `Map<shim_name, qualified_ref>`), collision detection.
- `forest::global::manifest` — manifest JSON → typed `Manifest { kind, tool, platforms, signature? }` decoder. Pure, total over malformed input (returns Result). Distinguishes `kind = "binary" | "external"`. Validates archive-mode invariants (e.g., `archive ≠ "none" ⇒ binary_in_archive` is `Some`; `kind = "external" ⇒ url` is `Some`; `binary_in_archive` is path-canonicalised — no `..`, no absolute).
- `forest::global::extract` — pure-ish: archive bytes + format + selector → either extracted-bytes or error. "Pure-ish" because real-world tar/zip libs touch the temp filesystem for streaming; the boundary is drawn so the **selection logic** (which entry to keep, path-traversal rejection, mode setting) is a pure function over the archive's entry list. Untar/unzip implementations live in the shell.
- `forest::global::eval_zsh` / `eval_bash` — pure string generators.
- `forest::global::path` — given `$PATH` env value, return whether shim dir is already present (for idempotency check generation).

**Effectful shell (the only code allowed to do I/O):**

- `forest::global::fs` — read/write `forest.cue`, `forest.lock`, shim files. Holds the file-lock primitive. All writes go through tempfile + fsync + rename.
- `forest::global::cue_eval` — evaluates CUE → JSON by shelling out to the `cue` binary (Q6.a). Same invocation pattern already used by `forest-sdk-codegen`. Per-process in-memory memoisation by `(path, mtime, size)` is allowed but no on-disk JSON cache is required by this spec. If `cue` is not on `$PATH`, the error message instructs the user to install it (auto-install is FU1, out of scope).
- `forest::global::cache` — content-addressable binary cache. Reader is `read_by_sha(sha) -> Path`; writer is `finalize(temp_path, expected_sha) -> Path` (verifies before rename — P3 hinges on this function).
- `forest::global::registry` — gRPC client calls. Returns bytes and parsed manifests; never makes resolution decisions.
- `forest::global::http_fetch` — HTTPS GET for external manifests. Returns body bytes + observed headers. Never decides whether to cache. TLS verification is mandatory (`https://` only, `rustls` defaults, no `--insecure` flag — that gap is a follow-up spec if needed). Redirects: follow ≤ 5; same-scheme only (no `https → http` downgrade).
- `forest::global::archive_io` — tar/zip extraction streaming, calls the pure `extract::select(...)` to decide which entry to keep, writes the selected bytes to a temp file. Path traversal rejection is duplicated here as a defense-in-depth check.
- `forest::global::exec` — `execv` the resolved binary path. The one syscall that ends the process.
- `forest::global::clock` — wall clock + monotonic clock, behind a trait `Clock` for stub injection.
- `forest::global::env` — read environment variables for shim mode.

The effectful shell **calls the pure core to decide what to do**, then performs exactly the actions the core dictated. The core does not import any of the shell modules (enforced by `cargo-modules` + a CI gate that fails on the wrong import direction).

### 1b.2 Properties catalog (test-based verification)

> **Kani has been dropped.** Per Architect direction the spec relies entirely
> on `proptest` + unit tests + acceptance tests against a live database, with
> structural / inductive arguments captured as the "How verified" column.
> Formal verification is not in this spec; if a future spec adds it, this
> catalog already factors the properties for re-targeting.

| ID | Property | How verified |
|---|---|---|
| P1 | For any `(lockfile, user_config, ref)`, `resolver::plan(...)` is total: returns `Plan`, never panics. | `proptest` with bounded inputs (entries ≤ 4, deps ≤ 4) — see `crates/forest/src/global/resolver.rs::tests::plan_is_total_on_arbitrary_empty_manifest_platforms`. |
| P2 | If `lockfile.get(org, name, ver, os, arch)` returns `Some(sha)`, the resolver's plan uses that sha. | Unit test `plan_for_tool_binary_with_lockfile_pin_uses_lockfile_sha`. |
| P3 | Cache-write invariant: `cache::finalize(bytes, expected_sha) → Ok(p)` implies `p = bin/<expected_sha>` and the bytes at `p` hash to `expected_sha`. | Inductive: `finalize` rejects on sha mismatch *before* `rename`; covered by `cache::tests::finalize_rejects_sha_mismatch` and `finalize_writes_and_read_by_sha_finds`. |
| P4 | `lockfile::parse(lockfile::serialize(L)) == L`. | `proptest` round-trip in `global::lockfile::tests::parse_serialise_round_trip`. |
| P5 | `shim_script_for` is byte-deterministic per the §1a.6 template. | Inline-fixture assertion + determinism test (`shim::tests::output_is_deterministic`). |
| P6 | `eval_zsh()` output contains the exact `case`-guard substring that makes double-sourcing safe. | `eval::tests::contains_idempotency_case_guard`. |
| P7 | Tool name validation rejects every input outside `^[a-zA-Z][a-zA-Z0-9._-]{0,63}$`. | `proptest` `accepted_names_match_regex` + 14 unit edge cases in `names::tests`. |
| P8 | _(reserved — signing property, deferred with §1a.3)_ | — |
| P9 | Catalog evaluation never emits a duplicate `shim_name`. | Unit + acceptance: `forest global add` refuses colliding shim filenames (§1a.8). |
| P10 | Concurrent `forest global add` cannot leave `forest.cue` partially written. | `fs::atomic_write` uses `tempfile + fsync + rename`; `fs::tests::atomic_write_replaces_existing_file` covers the in-process case; flock-based cross-process synchronisation is in the design (§1a.4) and is enforced through code review. |
| P11 | Manifest `kind ↔ plan-variant` correspondence: `Binary` ⇒ `FetchPlan::Registry`; `External` ⇒ `FetchPlan::Url`. | Structural: the `match manifest.kind` in `resolver::plan` is exhaustive. Tests `p11_binary_kind_never_produces_url_fetch` and `p11_external_kind_never_produces_registry_fetch`. |
| P12 | Archive entry selection is path-traversal safe (full statement in `extract::select` doc-comment). | `proptest` `p12_select_is_total_and_safe` + `p12_target_with_dotdot_always_rejected` in `extract::tests`. |
| P13 | The only path from an HTTPS body to exec runs through `cache::finalize`, which verifies sha. | Structural: `service::resolve_to_cached_path` calls `cache::finalize` before returning a path; there is no other code path that writes to `bin/<sha>`. Verified by reading the function. |
| P14 | URL allow-list: every URL Forest contacts during external download is `https://`, including redirect chain. | `service::http_get` constructs a `reqwest::redirect::Policy::custom(...)` callback that rejects non-https. The callback is small and inspected during review; integration testing via `forest tool hash` against real upstream URLs. |
| P15 | Org-catalog subscription does not bypass ban-list. | Acceptance test in `forest-server/tests/accepttest/global_tools_flow.rs` — adds an org subscription with a ban list, asserts banned shim is not created. |

### 1b.3 Verification tooling

- **`proptest`** — round-trip and oracle-style tests across the pure-core modules (lockfile, manifest, shim, name validation, extract).
- **acceptance tests** — live gRPC server + Postgres in `forest-server/tests/accepttest/global_tools_flow.rs` for server-side validation rules, shape persistence, and catalogue flows.
- **`cargo-deny`** — existing GPL-incompatibility check.
- **architectural review** — the `global::*` core modules SHOULD NOT import `service.rs` / `http_fetch.rs` / `cache.rs` (the effectful shell). Enforced by hand-review in this spec; if drift becomes a problem a `cargo-modules` lint can be added.

---

## 2. Files touched (overview, non-binding — actual list emerges from TDD)

Server:

- `interface/proto/forest/v1/registry.proto` — add `ListOrgTools` server-streaming RPC, `ComponentShape` enum, `ToolFacet`, `OrgToolEntry`, `PlatformInfo` messages. Extend `ComponentSearchResult` and `GetComponentDetailResponse` with shape + tool + methods + upstream URL fields per §1a.2e.
- `crates/forest-server/migrations/<date>_component_shape.sql` — add `components.shape TEXT NOT NULL DEFAULT 'component'` with the CHECK constraint from §1a.2e; backfill existing rows; drop the default after backfill so subsequent inserts must provide it.
- `crates/forest-server/src/services/component_aggregate.rs` — manifest validation rules 1–7 from §1a.2; compute `shape` from `(kind, tool, methods)` at publish time and persist it in the same transaction as the manifest row.
- `crates/forest-server/src/grpc/registry.rs` — implement `ListOrgTools` (scans `components WHERE shape IN ('hybrid_component','tool_binary','tool_external')`, streams `OrgToolEntry` with `shape` + `upstream_host`); extend `SearchComponents` + `GetComponentDetail` to populate the new fields.

(Signing-related files — `publisher_keys` table, `RegisterPublisherKey`/`TrustPublisherKey` RPCs, key management UI — DEFERRED with §1a.3.)

Client (`crates/forest`):

- `src/global/` — new directory: `mod.rs`, `resolver.rs` (hosts shared `NamespaceSource` trait — see Q7-followup), `lockfile.rs` (strict-mode global loader, see §1a.4), `shim.rs`, `names.rs`, `manifest.rs`, `extract.rs`, `cache.rs` (content-address writer + reader), `eval.rs`, `path.rs`, `fs.rs`, `cue_eval.rs` (shells out to `cue`, Q6.a), `registry.rs`, `http_fetch.rs`, `archive_io.rs`, `exec.rs`, `clock.rs`, `env.rs`.
- `src/cli/run.rs` — refactor onto the shared `NamespaceSource::ProjectComponents` impl from `crates/forest/src/global/resolver.rs` (FU2 — separable PR, in-scope for this spec's overall delivery). Ensures bare-name UX is symmetric between `forest run` and `forest global run`.
- `src/cli/global/{global_run.rs, global_remove.rs, global_list.rs, global_which.rs, global_update.rs, global_sync.rs, global_verify.rs, global_ban.rs, global_unban.rs, global_pin.rs, global_unpin.rs}` — new commands; extend `global_add.rs` for shim creation + alias + org-catalog subscription mode.
- `src/cli/eval.rs` — new top-level command (`forest eval zsh|bash`).
- `src/cli/tool/hash.rs` — new top-level `forest tool hash` dev helper. External tools are published via `forest components publish` (existing command, dispatched on `forest.cue` content).
- `src/cli/components/publish.rs` — extend to dispatch on `forest.cue`: `upload:` → existing binary path; `external:` → no build, no UploadBinary, just `PublishManifest` with `kind: "external"`.
- `src/user_config.rs` — replace the TOML reader with a CUE reader. Delete TOML write path. No migration code (legacy `forest.toml` is silently ignored per §1a.4).

SDK:

- `crates/forest-sdk/src/lib.rs` — extend `ComponentDescriptor` with `tool: Option<ToolFacet>`.
- `cue/forest-sdk/spec.cue` — add `#ForestTool` schema; extend `#ForestComponent` with optional `external?: #ForestExternal`; add `#ForestExternal` + `#ForestExternalPlatform`.
- `cue/forest-sdk/user_config.cue` — `#UserConfig` schema (NEW, used by `~/.config/forest/forest.cue`).
- `crates/forest-sdk-codegen/src/{ir,lower}.rs` — pass-through for the new fields.

Tests:

- `crates/forest/src/global/**/tests.rs` — unit + proptest + kani proofs.
- `crates/forest-server/tests/accepttest/global_tools.rs` — end-to-end: publish-as-tool (binary kind) → add → run → verify exec.
- `crates/forest-server/tests/accepttest/external_tool.rs` — end-to-end: publish external CUE manifest → add → run → verify download from a local HTTPS test server → exec.
- `crates/forest-server/tests/accepttest/org_catalog.rs` — end-to-end: publish two tools → `forest global add cuteorg` → both shims appear → ban one → ban'd shim disappears.

---

## 3. Phase-2 acceptance gate (this spec → TDD)

The spec is "Phase 1 done" when:

1. Sarcasmotron (Adversary) reviews this file and produces only nitpicks about wording — no concerns about missing edge cases, missing verification properties, or impossible-to-verify architecture.
2. The Architect (human) signs off on §1a.5 (CLI surface) and §1a.6 (shim format) — these are user-visible and reversal is expensive.
3. The Verification Strategy in §1b is acknowledged: proptest + unit + acceptance tests (Kani dropped — see §4 Q5 update).

Once the gate passes, Phase 2 starts. The first Phase-2 artefact is **the working example in [`examples/global-tools/`](../examples/global-tools/README.md)** — a complete end-to-end walkthrough that the implementation must reproduce verbatim. The example is built before the tests; the tests are derived from the example; the implementation is what makes the tests pass.

The example contains:

- The four artefact shapes from §1a.2e exercised individually (`TOOL_BINARY`, `HYBRID_COMPONENT`, `TOOL_EXTERNAL` × 3).
- Real upstream URLs for `ripgrep`, `jq`, `fd` (with placeholder sha256s that `forest tool hash` will fill in).
- A sample `~/.config/forest/forest.cue` showing both `dependencies` and `org_catalog` modes coexisting.
- The exact shell session — command + expected output — for every UX path the spec defines.

No implementation code is written until every test fails meaningfully.

---

## 4. Architect decisions (Q1–Q5 resolved)

- **Q1 — publisher key home**: DEFERRED. Signing follows a future org-keychain (cosign-equivalent) spec; no `forest auth keygen` in this spec. §1a.3 marks it as deferred.
- **Q2 — scoped publisher trust**: DEFERRED with Q1.
- **Q3 — global lockfile location**: `$XDG_STATE_HOME/forest/forest.lock` (default `~/.local/state/forest/forest.lock`). Captured in §1a.4. Rationale: it is regenerable from config, but is security-critical pinning data — `state` is the XDG bucket that matches both properties (not `cache`, not `config`).
- **Q4 — `forest eval zsh` scope**: Narrowly focused on PATH for now. `forest shell zsh` retains the other helpers. Extensibility deferred.
- **Q5 — Kani**: **Reverted — dropped.** The pure-core properties P1–P15 are validated by `proptest` + unit + acceptance tests instead (see §1b.2). The properties are still factored so a follow-up spec can retarget them at Kani or a similar checker without restructuring the code.

In addition, the Architect introduced two clarifications captured directly in §1a:

- **DC1 — CUE everywhere**: the external tool manifest (§1a.2b) and the user-global config (§1a.2c, §1a.4) are CUE files (`scaffolder.forest-tool.cue`, `~/.config/forest/forest.cue`) — not TOML — to stay consistent with the rest of Forest's CUE-native posture. A legacy `forest.toml` is migrated on first write.
- **DC2 — Signing deferral implications**: edge cases E5, E16 and property P8 are reserved-as-deferred placeholders to preserve numbering for the follow-up signing spec.

With these resolved, the spec is **locked** pending Adversary review. Phase 2 (failing tests first) begins on Adversary sign-off.

---

## 5. Architectural questions raised by the adversarial review

The Adversary surfaced four questions that I, the Builder, cannot decide unilaterally because each one materially changes module boundaries, runtime cost, or the deployment story. Trivial / decidable items from the review have already been folded into §1a–§1b directly; the four below are queued for Architect input. Until they are answered, the spec is **not** ready for Phase 2.

### Q6 — How does Rust evaluate CUE at runtime?

CUE is a Go-native tool. This spec moves user-global config to `forest.cue` and external tool manifests to `*.forest-tool.cue`. Today the codebase only invokes CUE at *build/publish* time (in `forest-sdk-codegen`), shelling out to a developer-installed `cue` binary. With this spec, `forest global add`, `forest global update`, `forest tool publish`, and (warm-path-sensitive) `forest global run` all need to evaluate or at least parse CUE.

Options:

- **Q6.a — Shell out to `cue`** on every read/write. Cheapest to implement; adds a runtime dependency on `cue` being on `$PATH` for every Forest user. Process-spawn cost on every read (~30–100 ms cold). **Warm path violates the ≤60 ms NFR unless `forest.cue` is cached as parsed JSON between invocations.**
- **Q6.b — Cache the evaluation result.** Read `forest.cue` once, evaluate to JSON, cache at `~/.cache/forest/global/config.json` keyed by sha of the cue file. Warm path reads the JSON; only writers re-evaluate. Mostly preserves Q6.a's simplicity, removes the warm-path cost. Cache invalidation = file mtime + sha.
- **Q6.c — Embed cuelang via cgo + Rust FFI.** Build `cue` as a `c-shared` library, link from Rust. Eliminates the external binary requirement. Large engineering lift; ties Forest to a specific cuelang version; complicates cross-compilation.
- **Q6.d — Use a Rust CUE parser.** None of the current Rust CUE crates are production-ready. Punt unless one matures.

**DECISION: Q6.a — shell out to `cue`.** `cue` is already an installed-tooling requirement for Forest contributors (existing codegen pipeline depends on it). For end users it is acquired the same way as any other build-tool prerequisite today; a follow-up item is to **auto-install `cue` on first Forest run** (likely via `forest global add` of `cuelang/cue`, eating Forest's own dogfood — recursive but neat). No JSON-result cache, no FFI, no Rust reimplementation. The warm path's CUE evaluation cost is paid by `forest global *` writers and by `forest global run` cold path; the shim warm path does NOT touch CUE (it only reads the lockfile + cache, see §1a.9 / §1a.9b).

Follow-up item recorded: **FU1** — auto-install `cue` binary on first Forest invocation. Not in this spec; not a blocker for Phase 2.

### Q7 — Where is the shim-name → qualified-ref index persisted?

`forest global run scaffolder` (bare name from a shim) needs to learn that `scaffolder ↔ cuteorg/scaffolder` without re-reading the registry. Options:

- **Q7.a — Embed the qualified ref in the shim body.** Already how the spec works: the shim invokes `forest global run cuteorg/scaffolder`. The shim filename is `scaffolder`; the body has `cuteorg/scaffolder`. Bare-name resolution is therefore **unnecessary on the shim path** — bare-name `forest global run` from a human terminal is the only case that needs an index, and it can do a synchronous scan of the shims directory (filename → first matching shim → read its body → extract ref). Total cost: one `readdir` + one short file read. Well within the warm-path NFR.
- **Q7.b — Maintain a separate `~/.cache/forest/global/index.json`.** Faster, more code, must stay in sync with the shim dir and with `forest.cue`.
- **Q7.c — Bake the index into forest.cue itself.** Adds an autogenerated section the user must not edit; breaks the "human-readable patch config" invariant.

**DECISION: Q7.a — the shim filename + shim body ARE the index.** Following the existing `forest run <command>` convention: bare-name resolution succeeds when unambiguous; on collision the user is told to use the fully qualified form `<org>/<name>[@<version>]`. For shims on PATH this collision is prevented at `forest global add` / `forest global sync` time (§1a.8) — Forest refuses to create a second shim with a colliding filename. For `forest global run <bare-name>` from a human terminal, the resolver scans the shim dir; if exactly one shim's filename matches, follow it; if zero matches, error; if more than one matches (only possible if the shim dir was manually edited), error and instruct the user to use `<org>/<name>`.

This brings global-tool invocation into UX symmetry with `forest run <command>` for components: same mental model, same fallback rule, no separate index file to keep in sync.

**Q7-followup — share the resolver implementation between `forest run` and `forest global run`.** Both commands solve the same shape: *bare-name → unambiguous qualified ref, falling back to explicit org/name on collision*. Code that lives separately today (`forest run` uses the project-level `forest.cue` + component dep graph; `forest global run` uses the shims dir) can share a single `Resolver` trait with two implementations of the source-of-truth:

```rust
// crates/forest/src/global/resolver.rs (new home for the shared trait)
trait NamespaceSource {
    fn lookup(&self, bare: &str) -> ResolverOutcome;
    //          ResolverOutcome = Unique(QualifiedRef) | Missing | Ambiguous(Vec<QualifiedRef>)
}

struct ProjectComponents(/* reads project forest.cue deps */);
struct GlobalShims(/* reads ~/.cache/forest/global/shims */);
```

The CLI command handlers (`forest run`, `forest global run`) become thin shells that pick the source, call `resolve`, and dispatch to the invocation path (component protocol vs argv passthrough). The shared error messages and the collision-→-qualified-form UX live in one place.

Phase 2 should write tests for `NamespaceSource` against both impls; if the existing `forest run` resolver is in a different shape today, the spec sanctions **refactoring it onto this trait as part of this work** (recorded as **FU2** — small refactor, in-scope but separable PR). Either re-use what's there or build it out — the spec is agnostic on which, as long as the resulting code path is shared and tested.

**Q7-followup-2 — resolution is shared, invocation is NOT.** Bare-name resolution returns a `QualifiedRef`; what `forest run` / `forest global run` do with that ref depends on the component's manifest. External tools (`kind: "external"`) have no `_meta/describe` and no method dispatch — they are plain binaries. The dispatcher therefore consults the manifest to select an invocation mode:

| `manifest.kind` | `manifest.tool` present | invocation mode |
|---|---|---|
| `binary` | no | component protocol (`./bin commands/<name> '<json>'`) — status quo for `forest run` |
| `binary` | yes (hybrid) | `forest run <name>`: component protocol. Shim / `forest global run`: argv passthrough. Same binary, two doorways. |
| `external` | yes | **argv passthrough only.** No describe, no methods. `forest run <name>` on an external-only component is equivalent to `forest global run <name>`. |
| `external` | no | rejected at publish: external manifests must carry a tool facet. (Added to §1a.2 validation rules.) |

In code, this is a one-line branch at the dispatch site:

```rust
match (manifest.kind, manifest.tool.is_some()) {
    (Kind::Binary, false) => component_protocol_invoke(...),
    (Kind::Binary, true)  if caller == ForestRun => component_protocol_invoke(...),
    _                     => argv_passthrough_exec(...),
}
```

This keeps the resolver pure (one shared module) and the divergence narrow (one match at the call site). Tests for `forest run` against an external-only ref must verify it falls through to argv passthrough.

### Q8 — Two-file atomicity for `forest.cue` ↔ `forest.lock`

Updating dependencies writes to two files in two different XDG directories. A crash between the two `rename` calls leaves the system in a state where forest.cue says "we want X@1.2.3" but forest.lock has no row for it (or vice versa).

Options:

- **Q8.a — Write lockfile FIRST, then config.** A half-applied state has dangling lock rows (extra rows that no config entry references). These are harmless: `forest global update` and `sync` are idempotent and tolerate orphans. The reverse ordering (config-then-lock) creates *missing* lock rows, which are NOT harmless — the next `forest global run` cannot verify the sha. **Pick lock-first.**
- **Q8.b — Single-file design.** Move the lockfile content into a section of `forest.cue` itself (`config.lock: [...]`). Single rename = single atomic point. Loses the read-only-ness of the lockfile and the ability to re-generate it without touching config.
- **Q8.c — Journal file.** Write intentions to a journal, apply, delete journal. Recovery on startup. Overkill for this surface.

**DECISION: Q8.a — lockfile-first ordering, guarded by `flock(2)`.**

Concretely: every write transaction acquires an advisory `flock(LOCK_EX)` on `~/.config/forest/.lock` (created `0600` on first use). Inside the lock:

1. Compute the new `LockFile` value.
2. Write `forest.lock` to a tempfile in `$XDG_STATE_HOME/forest/`, `fsync`, `rename`.
3. Compute the new `UserConfig` value.
4. Write `forest.cue` to a tempfile in `$XDG_CONFIG_HOME/forest/`, `fsync`, `rename`.
5. Release `flock`.

A crash between steps 2 and 4 leaves dangling lock rows (extra pins for entries the config no longer mentions). `forest global update` and `forest global sync` are idempotent and silently ignore orphan lock rows. A crash between steps 1 and 2 leaves nothing changed.

`flock` is advisory and process-scoped on Linux/macOS. Stale-lock detection: if a writer cannot acquire `LOCK_EX` within 30 s, it inspects the lock file's pid+timestamp marker (written by the current holder) and forces takeover if the pid is dead. This matches the existing pattern documented in §1a.10 E6.

### Q9 — Per-invocation re-hashing vs content-address trust on the warm path

§1a.9b proposes T1 (trust the content-addressable layout: the user owns the cache, the writer verifies sha before rename, the reader trusts). This makes the warm path fast (no re-hash). The original P3 wanted re-hash on every exec.

Options:

- **Q9.a — Adopt T1 + `forest global verify` as the explicit recovery knob.** Warm path ≤60 ms is achievable. Threat model excludes attackers with write access to `bin/` (they've already won). **This is what the spec now says.**
- **Q9.b — Re-hash on every exec.** Strictly safer; warm path NFR rises to ~200 ms for 20 MB binaries. Most CLI invocations are well under 20 MB (single-digit MB), so in practice ~30–50 ms. Could be acceptable.
- **Q9.c — Hybrid: re-hash only on first exec since last cold fetch.** Track in lockfile (`verified_at`). Complicated; mostly a worse Q9.a.

**DECISION: Q9.a — content-address trust on the warm path.** The hash is the cache key, nothing more on the warm path. The integrity guarantee on read comes from the writer-side verification (P3 — `cache::finalize` refuses to rename mismatched bytes into `bin/<sha>`). `forest global verify` (§1a.5) is the explicit user-invoked re-hash for paranoia or disk-corruption recovery; it is never on the warm path. Trust assumption T1 (user owns the cache dir) is recorded in §1a.9b and is part of the spec's threat model — attackers with write access to `bin/` have already won.

---

All four architectural questions are resolved. The spec is **fully locked**. Phase 2 (failing tests first) is unblocked.
