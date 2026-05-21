# 011 - Org-Level Install Button on the Components Page

**Status:** Phase 2 complete (failing tests written, implementation green). Phase 3 (adversarial review) complete; this document reflects the Phase-4 feedback loop — sections marked "post-review" below were rewritten after the initial draft pivoted from `pages/projects.html.jinja` to `pages/org_components.html.jinja` and the adversary flagged that the deeper sections still referenced the abandoned target.
**Depends on:** 004 (Projects + ForestPlatform), 007 (Tools/Components merged surface, install dropdown pattern), 008 (Project canonical home — source of the install-dropdown design we mirror).
**Driver:** Forest's `forest global add <org>` subscribes a workstation to *every* tool an organisation publishes (catalogue subscription, not per-tool install). Today that command is only surfaced on the individual *tool* and *project* detail pages — users who want to bulk-onboard an org's full toolchain have to first navigate into a specific tool to copy a command, then mentally strip the `/name` off. The org-scoped command belongs on the org-scoped *components* page, where users have already declared interest in "what does this org publish?".

---

## Problem

`forest global add <org>` is the highest-leverage onboarding command Forage
exposes: it wires up shims for the entire org's catalogue in a single line.
But the only places it's reachable in the UI are:

- `pages/project_detail.html.jinja:41–76` — *per-project* install dropdown.
  Renders `forest global add <org>/<name>` (scoped to *that* tool).
- `pages/component_detail.html.jinja:53–58` — same pattern, per-tool.

A user landing on `/orgs/{org}/components` (the org's catalogue surface —
the merged Components/Tools view after spec 007's revision) sees a search
form and a list of cards but **no way to bulk-onboard the whole catalogue**.
They either:

1. Click into a single tool, copy the per-tool command, strip the `/name` —
   guessing at a command they've never seen documented in-product, or
2. Skip the UI and run `forest global add` from memory / CLI help, or
3. Don't subscribe at all and copy-paste install commands one-by-one.

We're hiding a feature that already exists in the CLI behind a UI
that doesn't mention it.

---

## Goals

1. **Surface `forest global add <org>` on the org components page**
   (`/orgs/{org}/components`, rendered by `pages/org_components.html.jinja`).
2. **Match the existing install-dropdown design.** Visual, behavioural,
   and accessibility parity with the dropdown on
   `project_detail.html.jinja:46-75` — same `<details>`/`<summary>` shell,
   same green button colour, same `code_block` macro, same caret affordance.
3. **Hide when the org has nothing to install.** If the user lands on
   the page with no components published *and* no active search query,
   suppress the button — there's nothing for `forest global add` to
   actually install yet. Filtered-empty searches (`query` present,
   `components` empty) still render the button because the org *does*
   have a catalogue, just nothing matching the current filter.
4. **Zero new backend work.** This is a template-only change that uses
   the `org_name`, `components`, and `query` variables already passed
   to `pages/org_components.html.jinja`.

---

## Non-goals

- **Same button on every org-scoped surface.** Spec 007's "Implementation
  revision" merged the planned `pages/org_tools.html.jinja` into the
  existing `pages/org_components.html.jinja`, so there is no separate
  tools page to duplicate the button onto. The org's *projects* list
  (`pages/projects.html.jinja`) and *settings* sub-pages also stay
  untouched — the components page is the natural home because that's
  where users go to see what the catalogue contains.
- **Replacing the per-project install button on `project_detail.html.jinja`.**
  Per-tool and per-org installs are distinct intents and both stay.
- **Version-pinning the org subscription.** `forest global add` does not
  accept a version at org scope (only `<org>/<name>@<version>` is pinnable),
  so the dropdown shows a single command, not the two-option layout used
  on `project_detail.html.jinja:53-62`.
- **Showing per-user subscription state ("already subscribed" badge).**
  Server has no `forest.lock`-equivalent visibility yet (007 §"Out of
  scope" carries the same constraint).
- **Wiring the button to anything other than a copy-to-clipboard.** No
  server-side `/subscribe` POST; CLI remains the source of mutation.

---

## Architecture

This is a **template-only** change. No Rust, no routes, no DB, no proto.

### Files touched

| File | Change |
|---|---|
| `templates/pages/org_components.html.jinja` | Add `<details>` install dropdown in the existing header `flex justify-between` row (line 6), to the right of `<h1>Components</h1>`. Import `code_block` in the existing `{% from %}` line. Wrap the dropdown in `{% if components or query %}` so it disappears in the true-empty case only. |

### Template diff (post-review — matches what was actually shipped)

The current header at `org_components.html.jinja:6-8`:

```jinja
<div class="flex items-center justify-between mb-8">
    <h1 class="text-2xl font-bold">Components</h1>
</div>
```

becomes:

```jinja
<div class="flex items-center justify-between mb-8 gap-4">
    <h1 class="text-2xl font-bold">Components</h1>

    {# ── Org-scoped install dropdown (spec 011) ─────────────────
       `forest global add <org>` subscribes the workstation to every
       tool the org publishes. Mirrors the per-tool dropdown in
       `pages/project_detail.html.jinja` so the muscle memory carries
       over. Hidden when the org has nothing to install (no
       components AND no active query) — a filtered-empty search
       still surfaces the button because the catalogue isn't empty. #}
    {% if components or query %}
    <details class="relative shrink-0 group">
        <summary class="inline-flex items-center gap-1.5 px-3 py-1.5 bg-green-600 text-white text-sm font-medium rounded-md hover:bg-green-700 cursor-pointer list-none select-none">
            <svg class="w-4 h-4" …></svg>
            Install
            <svg class="w-3 h-3 ml-0.5" …></svg>
        </summary>
        <div class="absolute right-0 mt-2 w-96 max-w-[calc(100vw-2rem)] bg-white border border-gray-200 rounded-md shadow-lg z-10 p-4 space-y-3">
            <div>
                <p class="text-xs font-bold text-gray-500 uppercase tracking-wide mb-1.5">Subscribe to every tool in {{ org_name }}</p>
                {{ code_block("forest global add " ~ org_name, copyable=true, id="org-install-cmd") }}
            </div>
            <p class="text-xs text-gray-500">Subscribes your workstation to <span class="font-mono">{{ org_name }}</span>'s catalogue. Forest installs shims for every tool the org publishes and keeps them in sync.</p>
        </div>
    </details>
    {% endif %}
</div>
```

The `{% from %}` line at the top of the template gains `code_block`:

```jinja
{% from "components/ui.html.jinja" import badge, kind_badge, visibility_badge, tool_shape_badge, empty_state, pagination, code_block %}
```

(SVG `d`-attributes elided in the snippet above for readability — they
match `project_detail.html.jinja:48,50` verbatim.)

### Why `<details>`/`<summary>` (mirrors 008's design choice)

`project_detail.html.jinja` already uses native `<details>` for the
install dropdown. We mirror it deliberately:

- Keyboard accessible without JS (Enter/Space toggles).
- `class="relative"` on the parent + `class="absolute right-0"` on the
  panel overlays page content rather than pushing the components list
  down. Right-anchored so the panel doesn't overflow the viewport on
  narrow screens (`max-w-[calc(100vw-2rem)]` is the safety net).
- **Known limitation (carried from project_detail):** native `<details>`
  doesn't close when the user clicks outside it. The user has to click
  the summary again. Spec 008 ships with the same paper-cut; we don't
  introduce a new one. If product wants outside-click-to-close, do it
  uniformly across both dropdowns in a separate spec.
- The `group` modifier isn't currently load-bearing — no descendant
  uses `group-hover:`. Carried for textual parity with `project_detail`
  so future hover tweaks land in both places identically. (Adversary
  flagged this as dead weight; it stays, but is documented as such.)

### Why "Install" (not "Add", not "Subscribe")

`project_detail.html.jinja:49` switches between "Install" (for tools)
and "Add" (for components). At org level we don't know — the catalogue
may contain a mix. We pick **"Install"** because:

- The command is `forest global add` — which the CLI itself describes
  as installing global shims.
- Onboarding context: the org-level button is overwhelmingly used by
  *new* users who want their CLI to work; "Install" matches that mental
  model better than the more abstract "Subscribe" or "Add".

The label is technically a slight overclaim (the underlying operation
is closer to a subscription — shims are installed lazily on first
invocation). The dropdown's *header line* now leads with **"Subscribe
to every tool in {{ org_name }}"** so a careful reader sees the precise
semantics before they copy the command. (Post-review: the original
header said "Install every tool in …", which doubled down on the
overclaim instead of mitigating it.)

---

## Layout decisions (post-review — matches `org_components.html.jinja` layout)

- **Header row only.** No separate banner card above the search form —
  that would push the actual component list down for a feature most
  return users don't need on every visit. The dropdown is collapsed by
  default; cost-to-screen is one button.
- **Right-aligned in the existing flex row.** `justify-between` already
  positions the install button against the right edge. No new
  container, no grid; the existing layout is doing the work.
- **`gap-4` added to the flex row** as cheap insurance against the
  title and button kissing if a future translation makes "Components"
  much wider. With `justify-between` and only two children, the gap is
  visually a no-op today — kept defensively.
- **Search form (`org_components.html.jinja:10-13`) stays below the
  header.** It's its own row already, so the dropdown doesn't compete
  with it for horizontal space.
- **Empty state (`org_components.html.jinja:42-48`) replaces the
  component list, not the header.** When `components or query` is
  false, the header still renders (the `<h1>Components</h1>` doesn't go
  away) — but the install button inside the header is now also gone,
  so the empty-state hint ("Publish with forest components publish …")
  is the *only* call-to-action. That's intentional: if the user has no
  catalogue, the right next step is publishing, not subscribing.

---

## Edge cases

1. **`org_name` unset.** Cannot happen on this route — the handler at
   `routes/registry.rs:389` (`org_components`) extracts `{org}` from
   the path and renders the template only after `require_org_membership`
   succeeds. No `{% if org_name %}` guard.
2. **Org with zero published tools, no query.** Button hides. Empty
   state's "Publish with forest components publish ./my-component" CTA
   is the only suggestion shown. (Goal 3.)
3. **Org with zero published tools, active query.** Cannot happen
   today — without published tools there's nothing to filter. But the
   template guard (`{% if components or query %}`) handles it
   gracefully: button shows because `query` is truthy, dropdown
   suggests subscribing in case the org publishes something later.
   Acceptable.
4. **`page=2` on a truly empty org (no components, no query, page>1).**
   Button hides. The user shouldn't be on page 2 of an empty org —
   pagination doesn't generate this link — but a stale bookmark could.
   The hidden button matches the page=1 behaviour; no special handling.
5. **Viewer is not a member of the org.** Route 403s before render
   (`routes/mod.rs:154` "You don't have access to this organisation.").
   No special handling.
6. **Org name with shell-special characters.** Org slugs are
   `^[a-z0-9][a-z0-9-]*[a-z0-9]$`-ish (existing convention); no shell
   escaping concerns. The code-block content is plain text and minijinja
   auto-escapes HTML.
7. **Mobile / narrow viewport.** `w-96 max-w-[calc(100vw-2rem)]` and
   `right-0` keep the panel within the viewport. Smoke-tested visually,
   not asserted in template tests (no browser-driver infra in the
   project).
8. **Dark mode.** Panel uses `bg-white border border-gray-200`. The
   project's `input.css` inverts the Tailwind gray scale under
   `prefers-color-scheme: dark`. Same pattern as
   `project_detail.html.jinja:52` — consistent with existing UI debt.

---

## Verification architecture

### Provable properties

One testable invariant: **the rendered install command always equals
`forest global add <path_org>`**, where `<path_org>` is the org slug
from the URL — not the session's default org. This matters because the
button is the only path-vs-session collision point on the page; a
regression that wires the dropdown to a session variable would silently
ship the wrong org. Covered by
`org_components_install_command_uses_path_org_not_session_org`.

All other "verification" is presentation-glue assertion (string
present, element id present, button hidden when appropriate). No
proofs, no security boundary, no arithmetic.

### Purity boundary

Template renders downstream of a pure-side handler that already exists.
No new effectful code paths.

### Test surface

All tests added to
`crates/forage-server/src/tests/registry_tests.rs` under the comment
header `// ── Org-scoped install button (spec 011) ──`. A small helper
`create_test_session_with_orgs(&[…])` was added to
`crates/forage-server/src/test_support.rs` to support the path-vs-session
assertion.

Tests, in order of strictness:

- `org_components_renders_install_command` — happy path. Asserts the
  rendered HTML contains `forest global add testorg` and the element
  id `org-install-cmd`.
- `org_components_install_command_uses_path_org_not_session_org` —
  session is a member of `testorg` *and* `second-org`; request targets
  `/orgs/second-org/components`. Asserts the install command inside
  `<pre id="org-install-cmd">` equals `forest global add second-org`
  exactly. (Post-review tightening: original draft used a substring
  check against the whole document, which could false-pass if other
  content happened to contain the org name.)
- `org_components_install_caption_uses_path_org` — asserts the
  dropdown's *caption* (the body paragraph, not the code block) also
  renders the path org name. (Post-review addition: catches the "your
  org" hardcode regression that the command-only test misses.)
- `org_components_install_button_hidden_when_no_components` — true-empty
  catalogue. Asserts no `forest global add` string in the document.
  (This test trivially passed before the template change existed —
  documented here so a future reader doesn't misread "all four passed
  on red-gate" as suspicious. The companion `_renders_install_command`
  test is the positive control; together they form a meaningful guard.)
- `org_components_install_button_visible_on_filtered_empty_search` —
  catalogue is empty in this response *but* `q=nonexistent` is in the
  URL. Asserts the install button still renders.
- `org_components_empty_state_renders_when_no_components` — sister
  test to the hidden-when-empty assertion: when the button hides, the
  "No components published yet." copy must still render. Defends the
  `{% else %}` branch of the template against a future typo that eats
  the whole empty-state UI.
- `org_components_install_uses_native_details_element` — markup-structure
  guard (post-review C2-lite). The dropdown's `<summary>` is identified
  by its distinctive `bg-green-600` class (the nav dropdowns in
  `base.html.jinja` also use `<details>`/`<summary>`, so a naïve
  whole-document grep can't isolate it). Asserts: the install summary
  contains the visible "Install" label, and is nested inside a
  `<details>` parent. Catches a regression to `<div onclick="…">`
  which would silently break keyboard toggling.

No new mocks or fixtures beyond what existing `org_components_*` tests
already construct.

---

## Open questions

1. **Single or multi-org install?** Forest's CLI accepts only one org
   per `forest global add` call. Not changing here.
2. **Telemetry on the copy button?** No analytics infra in
   `code_block` today. Out of scope; opening this question would
   balloon the PR. Track separately if product wants conversion data.
3. **Outside-click-to-close on `<details>` dropdowns.** Native HTML
   doesn't do this. The existing `project_detail.html.jinja` dropdown
   has the same paper-cut. If product wants it, do it once for both
   dropdowns in a separate spec — adding it only here creates UX
   inconsistency.

*(Resolved during review: "Should the dropdown link to a tools
listing?" — moot, because the dropdown now lives ON the components
page itself, so the user is already looking at the catalogue.)*

---

## Acceptance criteria

- `GET /orgs/{org}/components` renders a green "Install" button to the
  right of the page heading **when the org has at least one component
  *or* an active search query**.
- `GET /orgs/{org}/components` on a truly empty org (no components, no
  query) renders the header `<h1>Components</h1>` and the "No
  components published yet." empty state — *no* install button.
- Clicking the button (or pressing Enter while focused) reveals a panel
  containing the command `forest global add <org>` as a copyable code
  block with id `org-install-cmd`, plus a caption that names the org.
- Visual diff against `project_detail.html.jinja`'s install dropdown
  shows matching button styling (colour, padding, icon, caret).
- All seven tests under "Test surface" pass; existing `org_components_*`
  tests remain green.
- No new clippy warnings, no new template lint warnings.

---

## Phase gate

Phase 1 (spec) — done. Phase 2 (failing tests → green) — done. Phase 3
(adversarial review) — done. Phase 4 (feedback loop) — this document
is the artefact, plus the test additions/tightenings and the caption
copy change. Phase 5 (hardening — clippy / static analysis) — passes
clean.
