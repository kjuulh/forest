# Spec 018: Release Pipeline Graph

Status: implemented and verified locally; stacked fork reviews and deployment pending.

## Problem

An individual release already exposes stages, policies, destinations, logs and
its specification. Those sections do not show dependency order or distinguish
parallel destinations at a glance. The same full node editor is available on
individual releases and both release overviews, alongside the existing charts.

## Behavioural contract

### Placement and source of truth

- A compact full-width node workspace sits above the existing release-detail
  content. Its canvas, toolbar and expandable inspector supplement those sections
  without replacing them or duplicating their mutation forms.
- Completed, active and future nodes all remain in the graph. Active changes the
  camera, not the dataset; Fit exposes the entire dependency graph.
- Project and organisation overviews use the identical server-rendered graph for
  the latest release per project. Existing compact flow charts and timelines remain.
  Overview graphs follow canonical timeline refresh, without extra polling loops.
- Timeline JSON adds stage `depends_on`, optional destination `stage_id` provenance
  and optional `graph_html` for the latest release per project. Older clients ignore
  these additive fields. No RPC, forest-server or database schema changes are needed.
- Overview graph projection fetches existing plan outputs and current policies,
  bounded to four concurrent release projections. It uses the same graph builder
  and component as details, including plan previews and policy nodes.
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

The graph's `data-live` flag is true while execution or an outstanding gate
remains unfinished, and false when the graph is terminal. Existing live-page
refresh updates the server-rendered graph without a full-page reload. Running
emphasis respects reduced-motion preferences.

### Markup, access and security

- The detail graph has `id="release-graph"`; overview instances namespace all DOM,
  ARIA and marker identifiers by project and artifact. Each exposes `data-live`.
- Nodes have lowercase `data-state` and logical `data-node-id="graph-node-N"`.
  DOM IDs are instance-prefixed; user-supplied labels never become SVG identifiers.
- Edges expose `data-from` and `data-to` referencing the logical node identifiers.
- SVG nodes remain keyboard-accessible anchors with existing stage, destination
  or policy targets as a progressive-enhancement fallback. Enhanced selection
  opens the inspector rather than relying on bottom-of-page jump links.
- Cards show their kind, environment, stage or destination label, state,
  destination summary, duration and available timing directly on the canvas.
  Plan output previews and scheduling, approval or failure reasons are visible
  in cards, not available only through hover or focus.
- The inspector provides Details, Plan and Logs controls. It shows the selected
  node's full context and fetched plan output; logs are available when the
  release has logs. One escaped inert template holds each plan's complete
  output, shared by its stage and destination nodes rather than duplicated in
  every card.
- Detail approval and rejection submit the existing authenticated forms with their
  original CSRF fields. Overview approval controls link to that release's real
  approval section; no duplicate forms or action endpoints are introduced.
- Active automatically frames current work as canvas space changes; Fit frames
  the whole graph. Fullscreen expands the workspace. Manual zoom and pan support
  exploration; live refresh preserves selection, camera and mounted logs.
- A legend and textual states explain the visual emphasis. Labels, reasons and
  plan output are escaped HTML, including hostile markup and quotes.
- The page uses no inline scripts, styles or event handlers. Behaviour and
  styling come from same-origin static assets under the existing CSP.
- A layered dependency layout supports multiple destinations, light and dark
  presentation, and narrow-screen access to both canvas and inspector.

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
- above-fold placement without dropping existing detail content, full overview/detail
  node projection equivalence and independent DOM identifiers across projects;
- unfinished and terminal `data-live` flags;
- safe graph identifiers, keyboard fallback targets, inspector and auto-fit
  controls, and reuse of the original authenticated forms without duplication;
- strict report-only CSP and absence of inline executable/style markup.

Browser interaction, automatic fitting on resize, pan and zoom, fullscreen,
inspector plan/log rendering, form submission and selection retention through
live refresh are integration gates, not established by these HTTP route tests.
The diagrams.net v3 suite is design intent, not evidence of a functioning
browser implementation. Screenshot capture uses headless Chrome over CDP with
its profile under the workspace `.workspace-cache/`, not Playwright.

## Deliberately not done

The workspace does not edit pipeline structure, introduce deployment actions,
replace the release timeline, add storage or migrations, or depend on a CDN.
Dependency topology and card positions are derived on the server. Both surfaces
consume the same graph component; overview HTML travels in the timeline JSON.

## Verification status

Verified 2026-11-21 against the real Forage test router with representative
release data and the strict CSP promoted from report-only to enforcing:

- release-render route checks passed, including the corrected aggregate-lock
  regression; state, multi-destination and hostile-input coverage is in
  `release_graph_tests.rs`;
- headless Chrome over CDP rendered the new graph widgets in light/dark desktop
  and a 390px mobile viewport under the enforcing strict CSP, without document
  overflow; the legacy timeline's existing inline positioning is unchanged;
- Plan showed the fetched output, Logs mounted the existing log viewer, and
  inspector approval submitted the original CSRF-protected form (204);
- Fit, Active, fullscreen entry/exit, native pointer pan, wheel zoom, keyboard
  selection and View stage all changed the actual browser surface;
- server-driven waiting → running → done updates retained the document, root,
  inspector, mounted logs, pinned selection and manual camera; terminal state
  stopped live updates;
- running animation was present normally and absent under reduced motion.
- shared full viewers rendered in both overviews alongside the retained compact
  flow charts and timeline cards, including two projects with the same release slug;
- automatic cameras followed live execution changes; policy-only context changes
  refreshed cards without requiring a stage transition;
- hostile title, destination/policy labels, failure reasons and plan output passed
  the actual timeline JSON render regression. Chrome mounted the hostile title
  and failure reason as text, with zero injected image/script/event-handler nodes.

Final screenshots: workspace `notes/full-details-light-desktop.png`,
`notes/full-overview-light-desktop.png`, `notes/full-overview-dark-mobile.png`,
`notes/full-overview-editor-light.png` and `notes/full-overview-hostile-inert.png`.
Editable design source: `notes/design/release-pipeline-v3.drawio`; the shared
viewer reuses its full node workspace with a compact initial viewport.
These artifacts are outside the repository. The temporary browser harness was
removed after verification; no test-only route or deployment shortcut ships.

Not established: the deployed origin, live backend data and production
approval side effects. The local browser's release data was a fixture.
