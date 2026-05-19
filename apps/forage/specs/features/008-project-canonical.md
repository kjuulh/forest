# 008 - Project as the Canonical Artefact Home

**Status:** Phase 1 — Spec, locked. Open questions answered; clean-empty-state policy added. Phase 2 (failing tests) unblocked.
**Depends on:** 004 (Projects + ForestPlatform), 007 (Components/Tools merged surface, native manifest rendering)
**Driver:** Components and projects describe the same thing from two angles. A project IS a component's repository in forest's data model (`forest.cue` declares both `project.name` and `forest.component.name`; convention has them equal). The UI today fragments this into two unrelated detail pages — `/components/{org}/{name}` and `/orgs/{org}/projects/{project}` — that share users, names, and underlying entities but render entirely different views. We want the project page to become the canonical home (GitHub-repo-style) and the component page to either fold into it or become a registry-search redirect target.

---

## Problem

After 007 the Components tab is the unified browse-and-detail surface for
every shape (`COMPONENT | HYBRID | TOOL_BINARY | TOOL_EXTERNAL`). But that
surface is divorced from the **project** entity. A user looking at
`/orgs/{org}/projects/forest-hello` (the deployment-focused project page)
sees an "Overview" tab with "No releases yet" + a small "Component
versions" list with no manifest, no install, no README. A user looking at
`/components/{org}/forest-hello` (the new merged Components surface) sees
install + invocation + releases + manifest but no triggers, no policies,
no deployment-pipeline status.

Same `{org}/forest-hello`, two unrelated UIs.

This violates the "one canonical URL per concept" principle. In
GitHub-mental-model terms: the project IS the repo, and everything (code,
releases, settings, deploy status, README) lives under one URL.

---

## Goals

1. **One canonical URL per artefact**: `/orgs/{org}/projects/{project}`
   is the home page for everything related to that project — what it
   does, what versions exist, how to install/use it, deployment status,
   triggers, policies.
2. **`/components/{org}/{name}` resolves into the canonical home** when
   a project with the same name exists. Registry-wide search at
   `/components` still works for discovery; it just redirects on click.
3. **Forge mental model** ("GitHub-repo with releases"): Overview tab
   shows README + description + install/usage; Releases tab shows
   version history; Settings tab shows visibility + triggers + policies
   + danger zone.
4. **No regression in deployment surfaces**: the current Continuous
   Deployment / Pipelines / Triggers / Policies actions stay reachable;
   they move to a Deployments tab (or stay on Overview, depending on
   shape — see §"Layout decisions").

---

## Non-goals

- Renaming `project` to `repository` in the data model. The label
  "project" stays; the UI just behaves repo-like.
- Auto-creating projects on component publish. That's a separate
  conversation (TASKS/022-auto-project-on-publish.md is the placeholder).
- Multi-component projects. The 1:1 case (component name ==
  project name) is what we support cleanly. The N-component case
  (a project that publishes multiple components) is acknowledged in
  §"Open questions" but out of scope for this spec.
- Settings management UI (visibility toggles, owner editing, danger
  zone). The "Settings" tab is a placeholder section showing the
  current state read-only; mutating it stays CLI-only for v1.

---

## What exists today

`/orgs/{org}/projects/{project}` already renders, with three tabs:

| Tab | Today | Notes |
|---|---|---|
| Overview | "Continuous deployment" with Pipelines/Triggers/Policies action chips, "No releases yet" empty state, "Component versions" list with platforms | Deployment-centric; no README, no description, no install copy |
| Releases | "No releases yet" empty state | Releases = deployments to destinations, not version publishes |
| Components | List of published versions with `Install: forest components add {org}/{name}@latest` | Most of what the new merged Components detail page shows — but in worse layout |

What's **missing** vs. the merged `/components/{org}/{name}` page:
- Shape badge (`tool` / `hybrid` / `tool-ext`) + visibility chip in header
- README (we render markdown; no project README field exists yet — see §"Data-model gap")
- Description as the project subtitle
- Tool install block (`forest global add ...`) when shape carries a tool facet
- Methods list (for hybrids), Upstream block (for externals)
- Distribution table (per-platform sha + size)
- Manifest pretty-printed JSON disclosure

---

## Architecture

### URL canonicalisation

| Today | After 008 |
|---|---|
| `/orgs/{org}/projects/{project}` | **Canonical home.** Tabs: Overview / Releases / Deployments / Settings. |
| `/components/{org}/{name}` | **303 → /orgs/{org}/projects/{name}** when a project with that name exists in `{org}`. Otherwise renders the standalone component-detail page (legacy / edge case). |
| `/components/{org}/{name}/{version}` | Keeps the existing legacy component-version-detail page. Direct version links don't get coerced through the Overview (since Overview is latest-stable only). |
| `/orgs/{org}/projects/{project}/releases` | Stays; folded into the Releases tab. |
| `/orgs/{org}/projects/{project}/components` | Folded into Overview (the single component case) OR a new `Components` sub-route (the multi-component case). |
| `/components` (global) | Stays as registry-wide search. Each card links to `/orgs/{org}/projects/{name}`. |
| `/orgs/{org}/components` (org list) | Stays — same role as `/components` but org-scoped. Each card links to the project. |

The two component-detail URLs survive as redirect-targets so external
links never 404.

### Tabs on the canonical project page

| Tab | URL | Always visible? | Purpose |
|---|---|---|---|
| **Overview** | `/orgs/{org}/projects/{project}` | yes | GitHub-repo-home pattern: Releases at the top, README below, Components below that (when present), Install/About cards in the sidebar. Empty out-of-the-box state shows a single Get-started panel — never a wall of empty cards. |
| **Releases** | `/orgs/{org}/projects/{project}/releases` | only when ≥1 version published | All-versions browse view. Two stacked sections: **Versions** (registry publishes) and **Deployments** (release artefacts to destinations). Hidden from the tab strip when no versions exist; the URL still resolves (renders the Deployments-only view today) for legacy link survival. |
| **Deployments** | `/orgs/{org}/projects/{project}/deployments` | yes | Pipelines / Triggers / Policies management — these are the **deployment settings** for the project. Always shown, even before any release, so users can configure their CD plumbing in advance. |
| **Settings** | `/orgs/{org}/projects/{project}/settings` | yes | Project-level metadata: visibility, owners, contracts, README (new field — see §"Data-model: project README"), danger zone. Read-only mutating UI for v1; copy-paste CLI commands replace inline edit forms. |

### Overview layout (GitHub-inspired)

Map the GitHub repo overview onto forest's concepts:

| GitHub | Forest |
|---|---|
| Repo header (name + About + watch/fork/star) | Project header (org/name + shape badge + visibility + description) |
| File tree (the central artefact list) | **Releases** — the central artefact list. Each release card == a registry version publish, with platform chips. |
| README rendered below the file tree | **README** rendered below the Releases section, sandboxed via `ammonia::clean` (existing) |
| (No equivalent) | **Components** section below the README — only renders when the project has multiple components, OR when the canonical component name differs from the project name. For the 1:1 case (project name == component name), the component IS the project and no separate section appears. |
| Sidebar: About, Releases summary, Packages | Sidebar: About card (latest version, total versions, kind, shape), Install block (for tools), Metadata (created/updated), Owners, Contracts |

**Top-to-bottom main column**:

1. **Releases** (top, the "files" equivalent) — cards with version,
   "latest" pill, platform chips, published-at timestamp. Top 5 + "Show
   all" disclosure. Same pattern as the 007 Releases section.
2. **README** — markdown rendered. Inherits from the canonical component
   (see §"Data-model gap: README"). Hidden when absent rather than showing
   a placeholder — GitHub's pattern is to omit the README block entirely
   when there's nothing to render.
3. **Components** (only when multi-component or name mismatch) — list of
   components linked to this project. Each entry is a card with the
   component's name, shape badge, latest version. Clicking goes to the
   merged Components detail (which redirects to this project page when
   names match, so this is mostly a forward-link helper for the multi-
   component case).

**Sidebar (right rail)**:

1. **Install** card — for tool shapes only. The `forest global add ...`
   copy block (with pinned variant). For non-tool components, an "Add to
   project" hint with `forest components add ...`.
2. **About** card — description, latest version, shape, kind, visibility.
3. **Metadata** card — created/updated timestamps, owners list.
4. **Distribution** card — compact per-platform sha + size table for the
   latest release. Full distribution table (with truncation) stays on
   `/components/{org}/{name}` (the legacy fallback page) and would be a
   click-through if a user wants a wider view; on the Overview it's just
   a sidebar summary.

Why merge Versions + Deployments into one "Releases" tab? They're both
"timelines" of the project, and GitHub conflates the two ("Releases" =
deploy-able tagged things). For forest, a version publish is the source
event and a deployment is the consequence. Stacking them with the
version timeline at top mirrors `git tag` → `git push` → deploy.

The current `/releases` URL keeps the Deployments-only view available for
linkability, but the Tab renders both sections.

### Data-model: project README

Projects gain a `readme: TEXT` column. This decouples the project home
page from any specific component's README, so:
- The 1:1 project ↔ component case has one source of truth: the project's
  own README. Components don't need to ship a README field for the
  Overview to render correctly.
- The multi-component case (deferred) has the right place to put the
  project-level narrative — distinct from per-component docs.

Pieces:
- **Migration**: add `readme TEXT NOT NULL DEFAULT ''` to `projects`.
- **gRPC**: `Project` message gains `readme` (string). `GetProject`
  returns it. `UpdateProject` accepts it.
- **CLI**: `forest project update --readme path/to/README.md` reads the
  file as UTF-8 text and sets `project.readme`.
- **`forest publish` writes README to BOTH destinations atomically.**
  On disk there's one `README.md` file; the publish syncs it to:
  1. The component's per-version files (existing behavior — gives the
     legacy `/components/{org}/{name}/{version}` page a version-pinned
     README snapshot for historical browsing).
  2. The project's `readme` field (new — gives the Overview a
     consistently-up-to-date README without requiring users to chase
     the latest component's file list).
  Opt out per-publish with `--no-readme`. If `README.md` is absent,
  publish proceeds without uploading (no error). The two updates happen
  in the same `CommitUpload` transaction so they can't diverge mid-flight.
- **Decoupled from publish**: `forest project update --readme PATH`
  updates only the project field, without requiring a new release.
  Useful for fixing typos / adding usage docs without a version bump.
- **Forage**: reads `project.readme` directly. The component `readme`
  field stays on `GetComponentDetail` for the legacy
  `/components/{org}/{name}` fallback page, but the project Overview
  uses the project field.

Validation:
- Limit to **64 KiB** at the gRPC boundary (covers any reasonable
  README; matches the manifest size cap from TASKS/018 for parity).
  Larger files are rejected at publish + at `project update --readme`.
- HTML/markdown sanitization happens client-side in forage via
  `ammonia::clean` (existing); the gRPC layer stores the raw markdown
  bytes so renderers downstream can choose their own sanitization
  policy.

### Cross-link bidirectional

- Project Overview links to `/components/{org}/{name}` (for users
  searching the global registry); the merged Components surface stays
  as the discovery view.
- Global `/components` search results link to
  `/orgs/{org}/projects/{name}` (the project home).

This means clicking through registry-wide search lands on the project
page, not the legacy component-detail page. The component-detail page
becomes invisible to navigation but reachable via the 303 redirect.

### Routes

| Route | Auth | Description |
|---|---|---|
| `GET /orgs/{org}/projects/{project}` | Member | **Overview** — README + description + install + latest-version distribution |
| `GET /orgs/{org}/projects/{project}/releases` | Member | **Releases** — versions + deployments (existing route, repurposed) |
| `GET /orgs/{org}/projects/{project}/deployments` | Member | **Deployments** — pipelines/triggers/policies (new route; lifts content from current Overview's "Continuous deployment" block) |
| `GET /orgs/{org}/projects/{project}/settings` | Member | **Settings** — read-only metadata view (new route) |
| `GET /components/{org}/{name}` | (Maybe) Session | **303** to `/orgs/{org}/projects/{name}` when project exists; current detail page otherwise |
| `GET /components/{org}/{name}/{version}` | (Maybe) Session | Renders the existing legacy component-version-detail page. No redirect into Overview — the Overview is latest-stable only. |
| `GET /orgs/{org}/projects/{project}/components` | Member | **Removed** (folded into Overview + Releases). 301 → Overview to preserve old links. |

### Templates

- `pages/project_detail.html.jinja` — **rewritten** Overview tab.
  Layout pattern matches `component_detail.html.jinja` from 007: header
  with badges, install (for tool shapes), shape-specific section
  (methods/upstream/invocation), Distribution table, raw manifest
  disclosure.
- `pages/project_releases.html.jinja` — **new**. Versions section
  (cards, GitHub-style) above Deployments section (existing per-
  destination state).
- `pages/project_deployments.html.jinja` — **new**. Pipelines + Triggers
  + Policies list, lifted from today's Overview.
- `pages/project_settings.html.jinja` — **new**. Read-only metadata
  view; mutating actions cite CLI commands.
- `pages/project_components.html.jinja` — **deleted** (the standalone
  Components tab inside a project goes away).
- `pages/component_detail.html.jinja` — **kept** but only reachable as
  a legacy/edge-case fallback. The redirect logic in `component_detail`
  handler skips it when a project exists.

### `ProjectDetailContext` (route handler)

The Overview handler needs the union of what `get_project_detail` returns
(today: project + projects list + environments + dest states + intents +
pipelines + component versions) **plus** what `get_component_detail`
returns (summary with shape/tool/methods/upstream_host, manifest_json,
readme). This is a lot to fan out; pragmatic approach:

```rust
// All futures run in parallel via tokio::join!
let (project_payload, comp_detail) = tokio::join!(
    fetch_project_payload(&state, &org, &project),
    fetch_optional_component_detail(&state, &org, &project),
);
```

`fetch_optional_component_detail` returns `Option<ComponentDetail>`;
when the project has no published component (yet), the Overview renders
without the install/manifest sections and shows "Publish your first
component" empty-state hint.

---

## Behavioral Contract

### Overview tab

Each section is **omitted entirely when its data is absent** — no empty-
state cards, no "No X yet" placeholders. A freshly-created project with
nothing published renders as a clean Get-started panel, not a wall of
empty sections. Once data exists, the section appears in its slot.

Main column (top → bottom, each conditional):

- **Releases section** — version cards with "latest" pill, platforms,
  published-at. Top 5 + "View all releases →" link to the Releases tab.
  - **Hidden** when zero versions are published. No empty card. The
    section header disappears too.
- **README** — sanitised markdown render. Fetched from the project's
  own `readme` field (new column on `projects`; see §"Data-model: project
  README").
  - **Hidden** when README is empty.
- **Components** (multi-component only) — list of additional components
  linked to this project. **Hidden in the 1:1 case** (project name ==
  single component name) and when there are zero named components.

Sidebar (right rail, each conditional):

- **Install** — `forest global add` (tool shapes) or
  `forest components add` (plain components). **Hidden** when no
  canonical component exists yet.
- **About** — shape, kind, visibility, description. **Hidden** when no
  canonical component (i.e., no shape/kind/description to show).
  Visibility is in the header, not duplicated here.
- **Metadata** — Created / Updated timestamps; Owners; Contracts. Always
  visible (the project itself has these even before any component
  publishes).
- **Distribution** — compact platform/sha/size table for the latest
  release. **Hidden** when no platforms.

Get-started panel:

- Renders only when **all** main-column sections are hidden (no
  releases, no README, no extra components). A single centered card
  with: "Publish your first version" + a `forest publish` copy block.
  Replaces what would otherwise be a near-empty main column.
- Disappears the moment any main-column section becomes non-empty.

Status codes:

- **403** when the user is not a member of the org.
- **404** when the project doesn't exist (existing behaviour preserved).
- **200 with degraded sections** when component-detail gRPC fails: the
  Releases section still renders (it comes from `list_artifacts`),
  README/Install/Distribution/About omitted; a small inline warning chip
  in the Metadata card surfaces the failure.

### Releases tab

- **Versions** (top section): cards with version, "latest" pill, platform
  chips, published-at timestamp. Sorted desc by semver. Top 5 + "Show
  all" disclosure. Same pattern as 007 Releases section.
- **Deployments** (below): existing per-destination state cards.
- **Empty Versions**: "No versions published yet. Run `forest publish`."
- **Empty Deployments**: "No deployments yet. Run `forest release create`."

### Deployments tab

- Lists Pipelines, Triggers, Policies. Action chips link to existing
  management endpoints.
- Empty state for each subsection.

### Settings tab

- Read-only `dl` of project metadata: org, project name, visibility,
  contracts, owners, created/updated timestamps.
- "Manage via CLI" hint with the relevant `forest project ...` commands.

### Legacy component-detail redirects

- `GET /components/{org}/{name}` where project `{name}` exists in `{org}`
  → **303 See Other** to `/orgs/{org}/projects/{name}`. Soft redirect;
  browsers/CDNs don't cache it aggressively, so reverting is painless.
- `GET /components/{org}/{name}/{version}` — **does NOT redirect.**
  Direct version links keep rendering the legacy component-version-
  detail page (since the Overview is latest-stable only). External
  links pinning a specific version stay deep-linkable.
- The `/components/{org}/{name}` redirect is conditional on the project
  lookup, NOT on shape. Any artefact whose name matches a project
  redirects to that project's Overview.
- When no project exists (rare: orphaned component), render the
  existing component-detail page unchanged.

### Component → project lookup

The Overview handler needs to know "does a project named `{name}` exist
in `{org}`?". Today the gRPC has `list_projects(org)` which returns names;
we'd hit it on every `/components/{org}/{name}` request to decide whether
to redirect. That's potentially slow.

Two mitigations:
- **Path-based optimisation**: when a user clicks a global-search card,
  send them DIRECTLY to `/orgs/{org}/projects/{name}` (404s land on the
  project page with a "this project doesn't exist" empty state — same
  UX as today). The `/components/{org}/{name}` URL only matters for
  external links.
- **Caching**: short TTL on `list_projects(org)` per session. The 4xx
  case (project doesn't exist) is rare; we don't need a cache miss
  penalty on it.

Spec prefers the path-based approach — change the link targets in
search results to project URLs directly, and let the legacy
`/components/{org}/{name}` URL absorb the cost of the lookup since it
only fires on external clicks.

---

## Edge Case Catalog

| ID | Case | Behavior |
|---|---|---|
| E1 | Project exists, no component published, no README | **Get-started panel only** — single centered card with `forest publish` copy block. No empty Releases/Components/Install sections. Deployments tab still visible (settings live there). |
| E1a | Project exists, no component, README populated | README renders; Get-started panel hidden. Sidebar shows Metadata only (no About/Install/Distribution — no canonical component). |
| E1b | Project exists, no component, no README, but Triggers/Policies configured | Get-started panel renders on Overview. Deployments tab shows the configured Triggers/Policies. |
| E2 | Project exists, component has no tool facet | Distribution table; sidebar shows `forest components add` |
| E3 | Project exists, component is HYBRID | Install block + Methods + Distribution |
| E4 | Project exists, component is TOOL_EXTERNAL | Install block + Upstream details + Distribution (only one platform usually) |
| E5 | No project, no component, URL hit | Project page renders 404 (existing) |
| E6 | No project, component exists at `/components/{org}/{name}` | Legacy component-detail page (no redirect) |
| E7 | Project + component, multi-component project | First/canonical component on Overview; rest accessible via a "Components" section in the sidebar (deferred — see §"Open questions") |
| E8 | Project visibility=private, user not member | 403 (existing) |
| E9 | Project has 50+ versions | Versions section in Releases tab uses top-5 + disclosure pattern |
| E10 | Component README is present but very long | Rendered in full; markdown safety via `ammonia::clean` (existing) |
| E11 | Component README has external img/script | Sanitised — `ammonia` strips. Inline links survive |
| E12 | Project + component but `get_component_detail` errors (e.g., gRPC `Unavailable`) | Overview renders project metadata; install/distribution/manifest sections omitted; `warn_default` logs the failure |
| E13 | Component manifest is malformed JSON | Distribution section omitted (parser returns None); raw JSON disclosure shows the fallback |
| E14 | `?version=X` query param passed to Overview | **Ignored**. Overview is latest-stable only. Direct version links go to `/components/{org}/{name}/{version}` (the legacy route). |
| E15 | Two components reference the same project name | Project Overview shows BOTH (rare). Component list in sidebar disambiguates. |

---

## Provable Properties (for tests)

| ID | Property | Test type |
|---|---|---|
| P1 | Overview renders 403 for non-members | route test |
| P2 | Empty project (no releases, no README, no extra components) renders the centered Get-started panel — no empty section cards | route test |
| P2a | Releases tab is hidden from the tab strip when no versions published | template assertion |
| P2b | Deployments tab is always visible (even with no versions) | template assertion |
| P2c | Sidebar Install/About/Distribution cards are hidden when no canonical component exists | template assertion |
| P3 | Overview renders the Releases section at the top of the main column | template assertion |
| P4 | Overview omits the README block entirely when README is empty | template assertion |
| P5 | Overview renders sanitised README markdown below the Releases section when present | route test |
| P6 | Overview omits the Components section in the 1:1 name-match case | template assertion |
| P7 | Sidebar Install block uses `forest global add` for tool shapes, `forest components add` for plain components | template assertion |
| P8 | `/components/{org}/{name}` 303s to project URL when project exists | route test |
| P9 | `/components/{org}/{name}` renders legacy page when project absent | route test |
| P10 | Releases tab shows both Versions + Deployments sections | route test |
| P11 | Deployments tab renders pipelines/triggers/policies | route test |
| P12 | Settings tab renders read-only metadata only | route test |
| P13 | `/orgs/{org}/projects/{project}/components` 301s to Overview | route test |
| P14 | Search-result links go to project URLs (not /components/...) | template assertion |
| P15 | Overview always renders the latest stable version regardless of any `?version=` query param | route test |
| P15b | Overview's "View all releases →" link points at `/orgs/{org}/projects/{project}/releases` | template assertion |
| P16 | Component-detail failure leaves Overview rendering Releases (Overview is partially-tolerant) | route test with failing mock |
| P17 | Markdown rendering escapes XSS (P11/P12 from 007 carry over) | route test |
| P18 | Overview context carries shape + tool fields when canonical component has them | mock-based route test |

---

## Test Plan

Tests live in `src/tests/platform_tests.rs` (project routes belong to
platform per the existing layout) and `src/tests/registry_tests.rs`
(redirect from `/components/{org}/{name}` belongs to the registry route
since that's where the redirect lives).

Mocks extend `MockPlatformClient` to add `list_projects_result` — already
exists — but we need the route handler to consult it for the redirect
decision. The `MockRegistryClient` doesn't need new behaviour.

---

## Implementation Order (after spec approval)

1. **Red**: write failing tests for P1–P14.
2. **Green** (in this order, each step keeps the suite green):
   1. Move existing `project_detail` handler to render the new Overview
      layout. Fetch component detail in parallel via `tokio::join!`;
      gracefully degrade when missing.
   2. Add `tool_shape_badge`, install block, Distribution table to the
      Overview template (reusing the macros from 007).
   3. Update the project-tab strip in `base.html.jinja` to show
      `Overview / Releases / Deployments / Settings`.
   4. Add `project_deployments` handler + template (lift the
      "Continuous deployment" content from today's Overview).
   5. Add `project_settings` handler + template (read-only `dl`).
   6. Update `project_releases` handler + template to render Versions
      above Deployments.
   7. Delete `project_components` route + template; redirect `301` to
      Overview.
   8. Update `/components/{org}/{name}` route to **303** to the project
      URL when a project with that name exists.
   9. Update the global `/components` search-result links to point at
      `/orgs/{org}/projects/{name}` directly (preserve the
      `/components/{org}/{name}` URL for external linkers).
3. **Refactor**: extract any shared rendering between
   `project_detail.html.jinja` and `component_detail.html.jinja` into
   a `components/_artifact_card.html.jinja` partial if duplication
   crosses ~30 lines.
4. **Adversarial review** (Phase 3) over the diff for E1–E15 + P1–P14.

---

## Resolved decisions (post-sign-off)

All open questions answered. The spec above already reflects each decision;
this section is the durable record for future readers.

- **URL grammar** — `/orgs/{org}/projects/{project}` is the canonical
  home. `/components/{org}/{name}` 303-redirects to it when a same-named
  project exists; otherwise renders the legacy component-detail page.
- **Tab structure** — Overview / Releases / Deployments / Settings.
  Releases merges versions + deployments timelines. Deployments houses
  the project's CD plumbing (pipelines/triggers/policies) — these are
  the **deployment settings**. Settings tab holds project-level metadata
  (visibility, owners, contracts, README).
- **Component detail folds into project** by redirect. The legacy URL
  keeps rendering when no project exists, to avoid breaking external
  links to orphaned components.
- **Overview layout (GitHub-inspired)** — main column: Releases (top,
  taking the "file tree" slot) → README (below) → Components (further
  below, only in the multi-component case). Sidebar: Install (for tools)
  / About / Metadata / Distribution-summary. Install copy block lives in
  the sidebar — matches GitHub's "Code → Clone" placement.
- **Multi-component projects** — deferred. v1 assumes 1:1 (project name
  == canonical component name). A future spec can re-enable the multi
  case; we may also choose to enforce 1:1 strictly at the data-model
  level later (user-noted possibility).
- **README source** — project-level `readme` field. New column on the
  projects table, new gRPC field on `Project` + `GetProject` +
  `UpdateProject`, new CLI flag `forest project update --readme PATH`,
  + auto-upload from `README.md` during `forest publish`.
- **`?version=X` on Overview** — Overview is always pinned to the latest
  stable version. No query-param scoping. The Releases tab is the
  canonical browse-all-versions page; the Overview's Releases section
  links to it via "View all releases →".
- **Redirect type** — 303 See Other for the `/components/{org}/{name}`
  → project URL canonicalisation. Soft commitment; reversible without
  cache pain.
- **Deployments tab name** — "Deployments". Short, clear, matches the
  existing `forest deployment` CLI noun.
- **Clean empty-state policy** — every section on Overview is hidden
  when its data is absent. No "No X yet" empty cards. A fresh project
  with no releases / no README / no extra components renders the
  centered Get-started panel only.

### Post-implementation amendments

- **Releases tab is now always visible** (was: "conditionally hidden when
  no versions"). Rationale: the tab houses CD plumbing — Pipelines /
  Triggers / Policies management — that a user reasonably wants to
  configure before any release. Hiding the tab also hid the CD config
  doorway. Implementation matches; spec text updated to reflect.
- **Tabs collapsed to `Overview / Releases`** (was: Overview / Releases /
  Deployments). "Deployments" was a misnomer — a deployment IS a
  release. The /deployments URL 303-redirects to /releases for legacy
  link survival.
- **Anonymous users skip the component → project redirect.** The redirect
  needs a session-scoped `list_projects` call to look up project
  existence; without an access token there's no way to authenticate
  that call. Anonymous traffic on `/components/{org}/{name}` lands on
  the legacy detail page directly. Documented in the route handler as a
  deliberate carve-out, not an oversight.

## Implications (for the implementation order section above)

The "Implementation Order" section now needs to additionally cover:

- Forest-side: migration adding `projects.readme TEXT`, RPC update,
  CLI `forest project update --readme`, `forest publish` README
  auto-upload. This is forest-server work, not forage-server.
- Forage-side: switch the Overview to read `project.readme` rather than
  the component's `readme` field. Sanitise via `ammonia::clean`.
- The conditional-tab rule (Releases tab hidden when no versions) lives
  in `base.html.jinja`'s project-tab strip alongside the existing
  conditional logic.

---

## Non-Functional Requirements

- **Performance**: Overview tab makes 1–2 gRPC calls (project payload +
  optional component detail) via `tokio::join!`. p95 render under 500ms
  against a warm forest-server.
- **Resilience**: any of the parallel calls can fail individually. The
  page renders whatever it got; missing data shows as section omission
  + a `warn_default` log.
- **Backwards compat**: `/components/{org}/{name}` URLs in the wild
  (PR descriptions, docs, bookmarks) keep working — they redirect to
  the project page. No 404.
- **Accessibility**: tab order matches visual order; sections use
  semantic `<h2>` + landmark roles where appropriate.
- **No client-side framework**: server-side MiniJinja, plain HTML +
  Tailwind, same convention as existing pages.
