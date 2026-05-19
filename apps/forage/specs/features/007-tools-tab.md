# 007 - Tools Tab (Package-Manager + Registry UI)

**Status**: Implemented as a **merged** Components/Tools surface (revised post-spec; see §"Implementation revision" at the bottom).
**Depends on**: 003 (BFF Sessions), 004 (Projects + ForestPlatform/ForestRegistry traits)
**Driver**: Global-tools work landed `ListOrgTools` + `OrgToolEntry` + `ComponentShape` + `ToolFacet` on the forest-server gRPC. Forage now needs a first-class browse surface for these so users can discover org-published tools without leaving the UI.

---

## Problem

Today, the only way to see what tools an organisation publishes is the CLI
(`forest global list`, `forest global add <org>`). Forage already has a
`Components` tab — but components and tools are a continuum (shape =
`COMPONENT | HYBRID | TOOL_BINARY | TOOL_EXTERNAL`), and mixing tools into
the general Components list buries the package-manager-style metadata that
makes tools useful (description, install command, supported platforms,
upstream URL for externals, version history with "latest" marker).

We want a dedicated **Tools** tab — feeling like crates.io / Docker Hub /
npm registry. Read-only initially; CLI remains the source of mutation.

---

## Scope

Mirror the existing components surface exactly — global + org-scoped + detail —
so the navigation muscle memory carries over.

| Existing components surface | New tools surface |
|---|---|
| `GET /components` — global registry search | `GET /tools` — global tools index |
| `GET /components/{org}/{name}` — global detail page | `GET /tools/{org}/{name}` — global tool detail |
| `GET /orgs/{org}/components` — org-scoped list | `GET /orgs/{org}/tools` — org-scoped list |
| `pages/components.html.jinja` | `pages/tools.html.jinja` |
| `pages/component_detail.html.jinja` | `pages/tool_detail.html.jinja` |
| `pages/org_components.html.jinja` | `pages/org_tools.html.jinja` |

This spec covers:

- **All three tool pages** above, layered to feel like a GitHub
  package-with-releases page (cards, not tables).
- **Cross-links** from tool → its component page (HYBRID shape only) and
  vice versa. A "Also a component →" link sits at the top of HYBRID tool
  detail pages; the corresponding "Also a tool →" link is added to
  `component_detail.html.jinja` when the shape is HYBRID / TOOL_*.
- **Pretty manifest** — pretty-printed JSON with light syntax highlighting
  (key/string/number tokens), not raw `<pre>` blob. Reuse via a new
  `pretty_json_block(json)` macro in `components/ui.html.jinja`.
- **gRPC integration** — `ListOrgTools` for both global + org-scoped lists
  (global iterates the user's orgs; reasonable first cut, since the RPC is
  already org-scoped — see Open Q5). `GetComponentDetail` reused for tool
  detail (a tool IS a component with shape != COMPONENT).
- **Navigation** — new "Tools" tab in `base.html.jinja` between Components
  and Destinations. Visible only at org level (matches components).
- **VSDD order**: spec → tests → code.

### Out of scope (future specs)

- Mutating the catalogue from UI: pin/ban/unban/sync (CLI-first per project
  convention). Tool detail shows the equivalent CLI commands as copy-paste.
- Tool publishing UI (CLI-only via `forest components publish`).
- Subscribing to org catalogues from UI (`forest global add <org>`).
- Showing per-user `forest.lock` state (no user-context server-side yet).
- Multi-org search across the whole registry (org-scoped only for now).
- Stripe billing tier limits on tool counts (orthogonal).

---

## Architecture

### gRPC plumbing

`ListOrgTools` is server-streaming on `RegistryService`. The wrapper trait
collects the stream into a Vec — list pages will never be huge in practice
(<= a few hundred tools per org) and the simpler signature lets us keep
`ForestRegistry` POJO-friendly.

```rust
// forage-core::registry

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub organisation: String,
    pub name: String,
    pub latest_version: String,
    pub shape: ToolShape,          // domain enum, not the proto one
    pub description: String,       // from ToolFacet.description; "" if absent
    pub argv_passthrough: bool,    // from ToolFacet.argv_passthrough
    pub upstream_host: String,     // populated for TOOL_EXTERNAL only ("" otherwise)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolShape {
    Hybrid,          // COMPONENT_SHAPE_HYBRID
    ToolBinary,      // COMPONENT_SHAPE_TOOL_BINARY
    ToolExternal,    // COMPONENT_SHAPE_TOOL_EXTERNAL
    Unknown,         // unspecified / future variants — render as "tool"
}

#[async_trait]
pub trait ForestRegistry: Send + Sync {
    // ... existing methods ...

    /// List tools published by an organisation. Filters server-side to
    /// shape in (HYBRID, TOOL_BINARY, TOOL_EXTERNAL).
    async fn list_org_tools(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<ToolSummary>, PlatformError>;
}
```

`GetComponentDetail` is already exposed; tool detail reuses it. Tool-
specific fields surface from `manifest_json` (parsed lazily in the route
handler — manifest is already a string we render, so this is incremental).

### Domain types (forage-core)

Defined above (`ToolSummary`, `ToolShape`). One conversion helper in
`forest_client.rs` maps proto → domain.

### Routes

| Route | Auth | Description |
|-------|------|-------------|
| `GET /tools` | Required (session) | Global tools index across orgs the user belongs to. Mirrors `/components`. |
| `GET /tools/{org}/{name}` | Required (session) + org membership | Global tool detail. Mirrors `/components/{org}/{name}`. Cross-links to `/components/{org}/{name}` if HYBRID. |
| `GET /orgs/{org}/tools` | Member of org | Org-scoped list, accessed via the org tab. Mirrors `/orgs/{org}/components`. |

URL grammar mirrors components exactly. `{name}` is the tool's component
name (slug, validated via existing `validate_slug`).

There is intentionally NO `/orgs/{org}/tools/{name}` (the URL would collide
with the global detail page semantically — one canonical detail URL per
tool, exactly like components today). The tab strip's "Tools" link goes to
`/orgs/{org}/tools` (the org list), and clicking any tool there navigates
to `/tools/{org}/{name}`.

### Templates

- `pages/tools.html.jinja` — **global** list. Mirrors `pages/components.html.jinja`.
- `pages/tool_detail.html.jinja` — **global detail** page. Mirrors
  `pages/component_detail.html.jinja` (3-col-main + sidebar layout).
- `pages/org_tools.html.jinja` — **org-scoped** list. Mirrors
  `pages/org_components.html.jinja`. Filter input when >5 tools.
- New macro `tool_shape_badge(shape)` in `components/ui.html.jinja` —
  styled chip: `[tool]` (TOOL_BINARY), `[hybrid]` (HYBRID), `[tool-ext]`
  (TOOL_EXTERNAL). Same visual language as existing `kind_badge`.
- New macro `pretty_json_block(json, id?)` in `components/ui.html.jinja` —
  pretty-prints a JSON string with class-based syntax highlighting
  (`.json-key`, `.json-string`, `.json-number`, `.json-bool`,
  `.json-null`). Tokenisation happens server-side in a small Rust helper
  (`forage-server::pretty_json`); the macro just wraps the pre-tokenised
  HTML. Avoids client-side JS dependencies; works without Prism/hljs.

### Navigation

In `base.html.jinja:119` insert a new tab between Components and
Destinations:

```html
<a href="/orgs/{{ current_org }}/tools" ...>Tools</a>
```

`active_tab == "tools"` highlights it. No changes to project-level tabs
(tools are an org-level concept).

---

## Behavioral Contract

### List view (`GET /orgs/{org}/tools`)

- **403** if the session user is not a member of `{org}`.
- **500** with `error_page` if `list_org_tools` returns a transport error.
- **Empty state** when zero tools: `"No tools published yet."` with hint
  `"Publish with forest components publish ./my-tool — see TASKS/018 for
  the manifest format."` Empty state never renders for transport errors.
- **Filtering** (UX): if `>5` tools, show a client-side filter input
  (same pattern as `pages/projects.html.jinja`). No server-side search in
  this spec — defer to a later iteration if the list grows.
- **Cards** show, in order:
  - Name (bold) + shape badge + version chip (`v0.1.0`) + upstream host
    chip on externals (`← github.com`)
  - Description (truncated to one line)
- **Sort** stable by name asc (server already returns highest-non-prerelease
  version per tool; we keep that ordering at the wrapper layer).
- **Each row links** to `/orgs/{org}/tools/{name}`.

### Detail view (`GET /tools/{org}/{name}`)

- **403** non-member of `{org}`.
- **404** when `GetComponentDetail` returns `not_found`.
- **303 redirect to `/components/{org}/{name}`** when the component
  exists but its `shape == COMPONENT_SHAPE_COMPONENT` — it's a regular
  component, not a tool; rather than 404, send the user to where they
  meant to go. (Reversing the earlier proposal: redirect feels better
  than 404 for shared `{org}/{name}` URLs.)

**Page layout** (top → bottom, by user importance):

1. **Header — Catalog metadata** (always at the top, package-manager idiom):
   - Org/name path, shape badge, latest version chip
   - Description (full, from manifest's tool facet)
   - Install block (copy-paste, with click-to-copy via existing JS pattern
     if present, otherwise plain `<pre>`):
     ```
     forest global add {org}/{name}
     ```
   - Pinned form as a smaller secondary block:
     ```
     forest global add {org}/{name}@{latest_version}
     ```

2. **Domain / Services / Ports** (shape-specific surface — the *what does
   this expose* part, more important than version history):
   - HYBRID → **Methods** section: list every method the component exposes
     (from manifest's `methods[]`). Each method renders as a row with name
     + brief description if available. This is the equivalent of a npm
     package's "API" section.
   - TOOL_EXTERNAL → **Upstream** block: full upstream URL (copy-able),
     archive type, binary path inside archive, sha256 (truncated chip
     with full-on-hover). The "ports" of an external tool = how the
     binary is fetched + verified.
   - TOOL_BINARY → **Invocation** block: `argv_passthrough: true` chip
     + a short prose hint ("Forwards all argv to the tool binary
     verbatim"). Minimal — TOOL_BINARYs are intentionally just a binary
     with a name.
   - All shapes → **Platforms supported** matrix (visible here, not only
     in the version history): which OS/arch combos the *latest* version
     ships for. Past versions' platform support sits in the Versions
     section below.

3. **Releases section** (GitHub-style cards, *not* a table) — sorted
   desc by semver. Each release card:
   - Version chip + "latest" pill on the highest non-prerelease
   - Published-at timestamp (right-aligned, muted)
   - Platforms list as small chips (linux/amd64, macos/arm64, …)
   - Anchor link `#v0.1.0` so users can deep-link to a specific release
   - Default render: top 5 cards + a "Show all (n more)" disclosure
     control. Avoids overwhelming the page when a tool has 20+ versions.

4. **Manifest section** (collapsed by default) — **pretty-printed JSON
   with light syntax highlighting** via `pretty_json_block(...)`.
   Power-user reference; bottom of the page.

5. **Cross-link strip** (in the right-rail sidebar, like component
   detail's "Metadata" / "Owners" / "Contracts" cards):
   - HYBRID → "Also a component" card with a link to
     `/components/{org}/{name}` and the methods count.
   - All shapes → "Source org" card with link to `/orgs/{org}/tools`
     (the org's tool catalogue).

This ordering matches how crates.io / npm / Docker Hub / GitHub-with-
releases structure a package page: what the package IS and how to use it
comes first; releases history is secondary; raw spec at the bottom.

### Reciprocal cross-link on component detail

`pages/component_detail.html.jinja` gains a small badge in the header:
when the component's shape is HYBRID, render a `"+ tool"` chip linking
to `/tools/{org}/{name}`. When the shape is one of the TOOL_* variants,
the canonical detail is the tool page (the existing component detail
should redirect there — symmetric to the tools-side redirect above).

### Tab strip

- The new tab renders **only** in the org-level chrome (when
  `current_org` is set and `project_name` isn't). Project-level tabs
  unaffected.
- `active_tab == "tools"` highlights it on both list + detail.

### Auth

- Same as `projects_list`: `require_org_membership(&state, orgs, &org)`
  before any gRPC call.
- The `access_token` is forwarded to gRPC via the existing Bearer
  middleware in `forest_client.rs`.

---

## Edge Case Catalog

| ID | Case | Behavior |
|---|---|---|
| E1 | Empty org (no tools) | Empty state with install hint |
| E2 | Org with mix of components and tools | Components tab shows components, Tools tab shows tools; no overlap |
| E3 | Tool with no description in manifest | Render "(no description)" in muted gray |
| E4 | TOOL_EXTERNAL list row | Show `← github.com` upstream chip (host only); full URL only on detail |
| E5 | Detail page hit for a non-tool component | **303 redirect** to `/components/{org}/{name}` |
| E5b | Component detail hit for a TOOL_* shape | **303 redirect** to `/tools/{org}/{name}` (the canonical detail page for tools) |
| E6 | gRPC `Unauthenticated` | Standard `internal_error` chain (existing pattern) |
| E7 | gRPC `PermissionDenied` | Same — won't normally happen since we pre-check membership |
| E8 | gRPC `Unavailable` (forest-server down) | `error_page` with "Service unavailable, try again." |
| E9 | Many tools (>50) | Renders fine; client-side filter handles search |
| E10 | Tool name with unusual chars (`-`, `_`, `.`) | `validate_slug` accepts; URL-safe by spec |
| E11 | Multiple versions, all prerelease | List view's `latest_version` reflects server's choice (highest non-prerelease, but if none exists, server may return empty — we render `"—"` in that case) |
| E12 | Stream interrupted mid-collect | Wrapper returns `PlatformError::Unavailable`; user sees retry hint |
| E13 | Detail manifest parse failure | Render header + releases; skip the manifest section gracefully (log a warning); pretty-printer is total over `serde_json::Value` so this only triggers on truly malformed input |
| E14 | Global `/tools` for a user with zero orgs | Empty state: "Join an organisation to discover tools" |
| E15 | Global `/tools` for a user with many orgs | Fans out `list_org_tools` across orgs concurrently (tokio::join_all); orgs that fail individually are skipped with a `warn_default` log; user sees the union of what we got |
| E16 | HYBRID tool's manifest has empty `methods[]` | "Methods" card hidden; "Also a component" cross-link still rendered |
| E17 | Component detail (non-HYBRID, non-TOOL_*) | Behaves as today; no cross-link badge |
| E18 | Pretty-printer encounters non-UTF8 in manifest | Server-side highlighter falls back to `escape_html(raw)` inside the same wrapper — never panics |

---

## Provable Properties (for tests)

| ID | Property | Test type |
|---|---|---|
| P1 | Org-scoped list route renders 403 for non-members | route test |
| P2 | List route forwards access token to gRPC | mock-based route test |
| P3 | Empty `list_org_tools` produces the empty-state template branch | route test + template snapshot |
| P4 | Shape badge renders the right label for each `ToolShape` variant | unit test on the macro / helper |
| P5 | `convert_tool_summary(OrgToolEntry)` maps every field exactly | unit test in forest_client.rs `#[cfg(test)]` |
| P6 | `/tools/{org}/{name}` 303-redirects to `/components/{org}/{name}` when shape=COMPONENT | route test |
| P6b | `/components/{org}/{name}` 303-redirects to `/tools/{org}/{name}` when shape=TOOL_* | route test |
| P7 | `latest_version` chip absent when server returns empty string | template branch test |
| P8 | URLs reject invalid slugs (both list-scope and detail) | route tests |
| P9 | Detail page surfaces install block with correct CLI command | template assertion |
| P10 | TOOL_EXTERNAL detail page shows full upstream URL; list shows host only | template assertions |
| P11 | `pretty_json::tokenize` is total — never panics on any string input | proptest |
| P12 | Pretty-printed HTML escapes `<`, `>`, `&` in string literals | unit test |
| P13 | Global `/tools` route fans out to every org the user belongs to | mock-based route test |
| P14 | HYBRID detail page renders "Also a component" cross-link | template assertion |
| P15 | Component detail (HYBRID shape) renders "+ tool" cross-link | template assertion |

---

## Test Plan

Per `CLAUDE.md` (forage): tests live in separate files.

- **Unit tests** in `forest_client.rs` `#[cfg(test)] mod tests`:
  - `convert_tool_summary_maps_all_fields` (P5)
  - `convert_tool_summary_handles_missing_facet` (E3)
  - Tool-shape conversion table for every proto variant
- **Unit tests** in `forage-server::pretty_json` (new module):
  - `tokenize_handles_all_value_types` (P11)
  - `tokenize_escapes_html_in_strings` (P12)
  - `tokenize_is_total` (proptest over arbitrary strings — P11)
- **Route/integration tests** in `src/tests/registry_tests.rs`:
  - `org_tools_list_renders_for_member` (P2, P3)
  - `org_tools_list_403_for_non_member` (P1)
  - `org_tools_list_empty_state` (E1)
  - `global_tools_index_fans_out_to_orgs` (P13, E15)
  - `global_tools_index_empty_state_for_user_with_no_orgs` (E14)
  - `tool_detail_renders_with_install_block` (P9, P10)
  - `tool_detail_redirects_to_component_for_shape_component` (P6, E5)
  - `component_detail_redirects_to_tool_for_shape_tool_binary` (P6b, E5b)
  - `component_detail_hybrid_shows_plus_tool_chip` (P15)
  - `tool_detail_hybrid_shows_also_a_component_card` (P14)
  - `tool_detail_invalid_slug_400s` (P8)
- **Mock** in `test_support.rs`:
  - Extend `MockRegistryClient` with `list_org_tools_result: Option<Result<Vec<ToolSummary>, PlatformError>>`
  - `get_component_detail_result` already exists; reuse for tool detail
    (so HYBRID/TOOL_* shape can be injected via the existing mock surface)

---

## Implementation Order (after spec approval)

1. **Red**: write all the failing tests above against empty trait
   method + route stubs + macro stubs.
2. **Green** (in this order, each step keeps the suite green):
   1. Add `ToolSummary` + `ToolShape` to `forage-core::registry`.
   2. Add `list_org_tools` to the `ForestRegistry` trait.
   3. Implement `convert_tool_summary` + `list_org_tools` in
      `GrpcForestClient` (collect the server-streaming response into a
      Vec at the boundary).
   4. Add `forage-server::pretty_json` module + tokeniser.
   5. Add `tool_shape_badge` and `pretty_json_block` macros in
      `components/ui.html.jinja`.
   6. Add the three new routes (`/tools`, `/tools/{org}/{name}`,
      `/orgs/{org}/tools`) + their handlers.
   7. Add the three new templates (`tools`, `tool_detail`, `org_tools`).
   8. Add the redirect logic on `/tools/{org}/{name}` (component shape
      → component detail) AND `/components/{org}/{name}` (TOOL_* shape
      → tool detail). Add `+ tool` chip in `component_detail.html.jinja`
      header for HYBRID.
   9. Add "Tools" tab in `base.html.jinja` (org-level chrome only).
3. **Refactor**: extract any shared rendering between
   `org_components.html.jinja` and `org_tools.html.jinja` into a
   `components/_pkg_card.html.jinja` macro if duplication crosses ~20
   lines. Otherwise leave them separate (small templates beat clever
   sharing).
4. **Adversarial review** (Phase 3): hand the diff to the Adversary
   reviewer for E1–E18 + P1–P15 coverage check.

---

## Non-Functional Requirements

- **Performance**: list view renders in <300ms p95 against a warm
  forest-server (no fan-out fetches; single `ListOrgTools` call).
- **Security**: membership check happens BEFORE the gRPC call. No org
  enumeration via 404 vs 403 timing (use the standard
  `require_org_membership` helper which already handles this).
- **Accessibility**: shape badges use both colour and text (no
  colour-only signal). Detail page tab order matches visual order.
- **Caching**: none in this spec; rely on tonic's H2 connection reuse +
  forest-server's already-tested response cache. Browser cache headers
  unchanged from existing pages.

---

## Open Questions for Sign-off

1. **List filtering**: client-side string match (matches projects
   pattern) or server-side once we add it to the RPC? **Proposal:
   client-side now; revisit when an org crosses ~200 tools**.
2. **Global `/tools` fan-out**: should the global page issue one
   `list_org_tools` per org the user belongs to (proposed, simple), or
   gain a new `ListMyTools` RPC server-side (better, but more code +
   another spec)? **Proposal: fan out client-side for now; track a
   future RPC in TASKS/022-list-my-tools.md if perf gets ugly.**

### Resolved during drafting

- ~~Page structure~~ — mirror components exactly: global `/tools`, global
  detail `/tools/{org}/{name}`, org list `/orgs/{org}/tools`. Same URL
  grammar, same template layering, same right-rail sidebar.
- ~~Tool ↔ component cross-links~~ — HYBRID tool detail shows "Also a
  component →" sidebar card; reciprocal "+ tool" badge in component
  detail header. TOOL_* component detail 303-redirects to tool detail.
- ~~Detail page order~~ — catalog (header) → domain/services/ports
  (shape-specific) → releases → manifest. Versions and manifest move to
  the bottom; catalog and shape surfaces lead.
- ~~Install block~~ — unpinned form is the primary copy block; pinned
  `@version` form sits below it as a smaller secondary block.
- ~~Manifest default~~ — collapsed by default, but pretty-printed +
  syntax-highlighted when expanded (server-side tokeniser, no client JS).
- ~~Releases section style~~ — GitHub-releases-style cards, not table.
  Top 5 + "Show all" disclosure.
- ~~Extra metadata~~ — kept minimal for v1. No download counts,
  maintainers, or advisories. Reserved as future-spec items.

---

## Implementation revision (post-spec, 2026-05-19)

Live verification of the dedicated `/tools` surface showed it was the wrong
abstraction: HYBRID artefacts ended up rendered twice with cross-link
chips and 303 redirects ping-ponging between `/components/{org}/{name}` and
`/tools/{org}/{name}`. A tool *is* a component with a different shape;
splitting the UI exposed server-side taxonomy to users for no benefit.

**The merged surface** replaces the spec's two-tab/two-route design:

| Spec said | Shipped |
|---|---|
| `/tools` global index | dropped (merged into `/components`) |
| `/tools/{org}/{name}` detail | dropped (merged into `/components/{org}/{name}`) |
| `/orgs/{org}/tools` org list | dropped (merged into `/orgs/{org}/components`) |
| Tools tab in nav | dropped (the Components tab is the single doorway) |
| 303 redirects between `/tools` ↔ `/components` based on shape | gone (one canonical URL) |
| `pages/tool_detail.html.jinja` + `pages/tools.html.jinja` + `pages/org_tools.html.jinja` | dropped (all sections folded into `pages/component_detail.html.jinja`, `pages/components.html.jinja`, `pages/org_components.html.jinja`) |
| `/components` redirect to `/tools` when shape=TOOL_* | gone |

**What survived from the spec**, unchanged:

- The detail page layout order: catalog header → Install (for tools) →
  shape-specific section (Methods / Upstream / Invocation) → Releases
  cards → pretty-printed Manifest (collapsed).
- The `tool_shape_badge` macro + per-shape `tool` / `hybrid` / `tool-ext`
  / `component` labels.
- The server-side `pretty_json::tokenize` JSON highlighter.
- `forage-core::registry::ToolShape` + `ToolFacet` + `ToolSummary`
  domain types.
- `ForestRegistry::list_org_tools` (kept on the trait + impl, no longer
  called by the routes — useful for a future tools-only filtered view).
- Server-streaming → Vec collection in `GrpcForestClient`.

**Why the change is a win**:

1. One canonical URL per artefact. Search bookmarks, links in PRs, and
   the URL bar all converge on `/components/{org}/{name}`.
2. The HYBRID case stops being a UX edge case — its `Install` block
   sits naturally above its `Methods` list on the same page.
3. The Components list shows the shape badge (`tool`, `hybrid`,
   `tool-ext`, `component`) so users can filter visually in their head;
   no need to switch tabs to find a tool published by their org.
4. ~250 lines of redirect + duplicate-template code deleted.

**What didn't change in the data layer**: forage-grpc still pulls
`ListOrgTools`, `OrgToolEntry`, `ComponentShape`, `ToolFacet` via the
symlinked-from-forest proto module. `ComponentSummary` gained `shape`,
`tool`, `methods`, `upstream_host` fields. The `forage-core` types stay
exactly as spec'd.

**Tests retained** (now exercise the merged surface):
- `components_list_shows_tool_shape_badge_for_tool_binary`
- `components_list_shows_upstream_host_for_external`
- `component_detail_renders_install_block_for_tool_binary`
- `component_detail_no_global_install_for_plain_component`
- `component_detail_hybrid_shows_methods_and_install`
- `component_detail_external_shows_upstream_section`

Plus all the unit tests for `ToolShape`, `convert_tool_summary`,
`convert_shape`, `pretty_json::tokenize`.

**Live verified end-to-end via Playwright** (2026-05-19): registered a
user, created an org, published `forest-hello` (shape=tool_binary) via
the streaming UploadBinary RPC, browsed to `/orgs/{org}/components`
(saw the `tool` shape badge), clicked through to
`/components/{org}/forest-hello` (saw the Install copy block,
Invocation section, Releases card with `latest` pill, expanded the
pretty-highlighted Manifest), and verified the global `/components`
page shows the tool with its shape badge.
