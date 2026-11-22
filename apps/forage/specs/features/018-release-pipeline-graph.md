# Spec 018: Release Pipeline Graph

Status: shared workspace deployed and verified on dev/prod; optional Graph-tab navigation and compact header verified in the browser, awaiting fork rollout.

## Problem

An individual release already exposes stages, policies, destinations, logs and
its specification. Those sections do not show dependency order or distinguish
parallel destinations at a glance. Release lists must remain timeline-first;
the larger project workspace belongs in an optional Graph tab.

## Behavioural contract

### Placement and source of truth

- A full-width node workspace sits above the existing release-detail content.
  Compact breadcrumbs/title/metadata and a non-duplicating graph toolbar leave
  more room for the canvas while preserving every original detail section/form.
- Completed, active and future nodes all remain in the graph. Active changes the
  camera, not the dataset; Fit exposes the entire dependency graph.
- Project navigation exposes a **Graph** tab beside **Releases**. Only that
  opt-in page shows the high-resolution environment/destination workspace and
  expanded-active pipeline viewers. Default project Releases and all-project
  overviews show the original timeline/disclosures without graph requests.
- Environment lanes place a compact environment card above rows of destinations,
  wrapping after four targets with shared orthogonal connectors and no crossings.
- Each physical destination has one card per `(environment, name)`, retaining
  current completed and incoming operations together rather than rendering a card
  per operation. Additional pending operations are counted; the inspector retains
  every operation's provenance, state, reason and timing. A newer completion
  timestamp never hides queued work. Plan completion is not deployment success,
  and missing operation provenance remains explicit.
- The Graph tab's environment view contains no pipeline stage nodes or inferred
  promotion dependencies. Logical edges connect environments to destinations.
- On the Graph tab, expanding an active entry lazily loads its full pipeline.
  Collapsed or initially inactive entries make no graph request. An opened
  pipeline keeps its final update until collapse.
- Original timelines and stage disclosures remain. The lower-resolution overview
  flow and per-project top-level pipeline viewers are removed.
- Timeline JSON adds optional top-level `graph_html` only with `include_graph=true`;
  only the project Graph page opts in. The projection adds zero upstream RPCs.
- The authenticated lazy fragment evaluates policies only for the requested latest
  intent. Neither overview response fetches nor embeds plan output. The inspector
  fetches one plan only when Plan is opened; stale requests are aborted.
- Existing additive stage `depends_on` and optional destination `stage_id` fields
  remain compatible. No RPC, forest-server or database schema changes are needed.
- The latest release intent supplies stages and their explicit `depends_on`
  edges. Independent branches are not joined by incidental display order.
- Intent steps identify destination execution by `stage_id`, not only by
  environment or destination name. Plan and deploy steps for the same
  destination remain separate executions with separate states.
- Stage summaries count completed destination runs against all destination runs
  for that stage. Plan and deploy counts remain separate even when they target
  the same destinations.
- A destination's current state does not overwrite a historical intent step,
  including when that current row belongs to a different release or artifact.
- Configured destinations remain visible when no execution has started.
- Stage, approval, soak and other policy information remains attached to its
  actual environment and execution context. Unknown states are not presented as
  successful work.

### State and live updates

Nodes distinguish `running`, `next`, `queued`, `waiting`, `blocked`, `failed`,
`done`, `skipped`, `cancelled` and `superseded`. Reasons distinguish scheduling
and approval waits from failures. A pending node with satisfied dependencies
is next; an unfinished dependency prevents it from being next, and a failed
prerequisite blocks it. Terminal work does not become running because the
same destination later receives a different release.

Dependency readiness retains the aggregate stage state as well as its child
runs and approval gates. A running plan held by a release lock still prevents
a dependent deploy from becoming next when a reported destination plan has
already succeeded. Visual fan-out through completed children does not erase
the unfinished aggregate dependency. A passed policy likewise does not bypass
an outstanding plan approval.

The graph's `data-live` flag marks unfinished execution. Details poll every ten
seconds only while visible, on-screen and unfinished. Canonical overview refreshes
coalesce events, never overlap, and start at most once per ten seconds while the
document is visible. Only expanded visible pipelines refresh their lazy fragment.
Running emphasis respects reduced-motion preferences.

### Markup, access and security

- The detail graph has `id="release-graph"`; overview instances namespace all DOM,
  ARIA and marker identifiers by project and artifact. Each exposes `data-live`.
- Nodes have lowercase `data-state` and logical `data-node-id="graph-node-N"`.
  DOM IDs are instance-prefixed; user-supplied labels never become SVG identifiers.
- Overview destination cards expose a stable `data-node-stable-key` for the
  physical target, independent of release identity, state and timestamps.
  Selection and focus use this key across refreshes; other nodes retain their
  existing selection identity.
- Edges expose `data-from` and `data-to` referencing the logical node identifiers.
- SVG nodes remain keyboard-accessible anchors with existing stage, destination
  or policy targets as a progressive-enhancement fallback. Enhanced selection
  opens the inspector rather than relying on bottom-of-page jump links.
- Cards show their kind, environment, stage or destination label, state,
  destination summary, duration and available timing directly on the canvas.
  Scheduling, approval and failure reasons are visible directly in cards; details
  can also display their already-fetched plan previews.
- Pipeline inspectors provide Details, Plan and Logs controls. Overview Plan text
  loads on demand from the existing same-origin endpoint into `textContent`.
  Only the current fetched plan is retained. Empty metadata templates identify
  plan stages without carrying their output. The global workspace has no Plan/Logs.
- Detail approval and rejection submit the existing authenticated forms with their
  original CSRF fields. Overview approval controls link to that release's real
  approval section; no duplicate forms or action endpoints are introduced.
- Desktop global overviews initially Fit the entire workspace; Active frames the
  active environment and its destination neighborhood, not a single card.
  Pipeline workspaces retain their current-work framing. Camera bounds use each
  rendered card's actual dimensions, including compact environment cards.
- Fullscreen expands the workspace. Manual zoom and pan support exploration;
  live refresh preserves selection, camera and mounted logs. Narrow screens
  retain readable navigation without document overflow.
- A legend and textual states explain the visual emphasis. Labels, reasons and
  plan output are escaped HTML, including hostile markup and quotes.
- The page uses no inline scripts, styles or event handlers. Behaviour and
  styling come from same-origin static assets under the existing CSP.
- A layered dependency layout supports multiple destinations, light and dark
  presentation, and narrow-screen access to both canvas and inspector. The global
  workspace has more horizontal and vertical room without widening the timeline
  or changing the pipeline viewport.

## Verification mapping

`crates/forage-server/src/tests/release_graph_tests.rs` requests the actual
release routes through the existing mock platform boundary. It covers:

- every listed state and satisfied, unfinished and failed prerequisites;
- aggregate plan-lock and approval holds despite completed children or passed
  policies;
- per-stage destination association and summary counts, parallel branches,
  explicit edges and isolation from unrelated current destination state;
- environment and duration in visible cards, visible wait and policy reasons,
  and one shared escaped plan-output template;
- configured destinations and escaped hostile text;
- above-fold detail placement, global environment/destination projection, full
  detail/lazy-fragment equivalence, and operation provenance within target cards;
- one target card retaining completed and queued records with stable identity;
- actual rendered card bounds across two environments and more than four targets,
  including wrapped rows and retained logical environment-to-target edges;
- unfinished and terminal `data-live` flags;
- safe graph identifiers, keyboard fallback targets, inspector and auto-fit
  controls, and reuse of the original authenticated forms without duplication;
- strict report-only CSP and absence of inline executable/style markup.
- opt-in projection with zero extra RPCs, lazy per-intent policy evaluation without
  plan output, authorization before upstream work and cross-project slug isolation;

Browser interaction, automatic fitting on resize, pan and zoom, fullscreen,
inspector plan/log rendering, form submission and selection retention through
live refresh are integration gates, not established by these HTTP route tests.
The diagrams.net suites, including the v6 readable overview, are design intent,
not evidence of a functioning browser implementation. Screenshot capture uses
headless Chrome over CDP with its profile under the workspace
`.workspace-cache/`, not Playwright.

## Deliberately not done

The workspace does not edit pipeline structure, introduce deployment actions,
replace the release timeline, add storage or migrations, or depend on a CDN.
Dependency topology and card positions are derived on the server. Both surfaces
consume the same graph component; overview HTML travels in the timeline JSON.

## Verification status

- 66 release-route checks and two org-release checks passed, covering the new
  Graph-route session/membership boundary, state/provenance, opt-in cost,
  hostile strings, stable target identity and non-overlapping wrapped lanes.
- 101 frontend tests passed across API compatibility, visibility-aware prefetch,
  ten-second scheduling, lazy graph lifecycle and lane state. Bundle, CSS and
  controller syntax checks passed.
- Chrome/CDP: the readable global overview rendered nine nodes, all inside its
  canvas with no overlapping cards. Desktop defaults to Fit; Active framed the
  entire prod environment. An expanded active entry rendered twenty pipeline
  nodes and fetched the selected plan on demand. Light/dark desktop and 390px
  mobile screenshots were captured without document overflow.
- Earlier shared-editor browser checks covered native pan/zoom, fullscreen,
  Plan/Logs, original approval submission, strict graph CSP, inert hostile
  strings and inspection retention across live updates. Deterministic lifecycle
  tests establish hidden-tab pausing; CDP did not establish a real hidden tab.
- Navigation proof: all-project and default project Releases render zero graphs
  and fetch timeline JSON without the graph flag. Native Graph/Releases clicks
  and browser Back switch view-specific preserved elements correctly. Graph
  mode rendered eight environment nodes and an expanded sixteen-node pipeline;
  its Plan tab loaded output on demand.
- Detail graph began at 232px with a 32px toolbar in the desktop fixture.
  Duplicate context is hidden normally and restored in native fullscreen.
  Detail, Releases and Graph views had no document overflow at 390px.

Editable design sources: workspace `notes/design/release-pipeline-v3.drawio`
(card/editor fidelity), `notes/design/release-pipeline-v5-environments.drawio`
(global status and expanded pipeline placement),
`notes/design/release-pipeline-v6-readable-overview.drawio` (readable environment
lanes), and `notes/design/release-pipeline-v7-navigation.drawio` (opt-in Graph tab
and compact detail header), opened in diagrams.net.

The shared renderer was verified against actual dev/prod revision `a2e01b20`:
production showed nineteen global nodes without painted-card collisions, real
Plan output as inert text, and an authenticated lazy fragment with no embedded
plan text. No production release-action forms were submitted. This navigation
revision still awaits its own pinned review and rollout.
