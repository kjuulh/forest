# 011 - Environment Reordering (Drag-and-Drop)

**Status:** Phase 1 — Spec, locked. Phase 2 (failing tests) unblocked.
**Driver:** Environments on the Destinations page (`/orgs/{org}/destinations`) carry a `sort_order` field that controls render order, but once created there is no way to change it from the UI. The CLI sets the order at creation time and that value is sticky. Users have to delete-and-recreate (or hit gRPC directly) to fix ordering — both bad. The gRPC backend already exposes `EnvironmentService.UpdateEnvironment` with optional `sort_order`, so the frontend is the only gap.

---

## Problem

On the Destinations page, environment cards display "order: N" but the value is read-only. Common workflows that require reordering:

- Adding a new environment that should slot between two existing ones.
- Renaming a pre-prod tier that pushes its order out of sequence.
- Onboarding to forage with environments that were imported in the wrong order.

There is no admin affordance to fix any of these without going through the CLI / gRPC.

---

## Goals

1. **Admins can reorder environments from the Destinations page** by dragging cards into the desired order. The new order persists immediately.
2. **No new gRPC plumbing.** `EnvironmentService.UpdateEnvironment` already supports `sort_order`; we extend the `PlatformClient` trait with `update_environment` and wire the existing call.
3. **Non-admins see a static list** — no drag handles, no visible reorder UI.
4. **Failure modes are recoverable.** If the server rejects a reorder, the UI reverts to the server's authoritative order on next page load (no optimistic-only state).

---

## Non-goals

- Renaming or editing environment description from this UI (UpdateEnvironment supports description, but that's a separate feature).
- Bulk reorder / re-numbering API on the gRPC side. We commit one env-move at a time, server-side persists whatever `sort_order` we send.
- Mobile-touch drag interactions (the page is admin-only, desktop-first).
- Resolving sort_order collisions: if two envs share a sort_order (legacy data), tie-break is undefined; the user is expected to drag one of them to fix it.

---

## Behavioural Contract

### `PlatformClient::update_environment`

Trait method on `forage_core::platform::PlatformClient`:

```rust
async fn update_environment(
    &self,
    access_token: &str,
    id: &str,
    description: Option<&str>,
    sort_order: Option<i32>,
) -> Result<Environment, PlatformError>;
```

- **Preconditions:** `id` is a non-empty environment id known to the server. At least one of `description` / `sort_order` is `Some` (no-op updates allowed but uninteresting).
- **Postconditions:** Returns the updated `Environment`. On success the server's `sort_order` matches the requested value.
- **Errors:** Maps gRPC `NotFound` → `PlatformError::NotFound`; `PermissionDenied` → `PlatformError::Unauthorized`; everything else → `PlatformError::Other`.

### Route: `POST /orgs/{org}/destinations/environments/{id}/order`

- **Auth:** session required; `require_org_membership(&org)` + `require_admin(current_org)`. Non-admins get 403.
- **CSRF:** form must include `_csrf` matching the session token, else 403.
- **Form body:**
  - `_csrf: String`
  - `sort_order: i32`
- **Success:** Calls `platform_client.update_environment(token, id, None, Some(sort_order))`. Redirects 303 → `/orgs/{org}/destinations`.
- **Failure:** Returns an `error_page` with the upstream error message.

### UI: Destinations page

For admins:

- Each environment card grows a drag handle (⋮⋮ icon, left of the status dot).
- The list of env cards is wrapped in a single container with a stable id (`#env-list`). Each card gets `data-env-id="{{ env.id }}"`.
- On `drop`, JS computes the new `sort_order` for the moved card and POSTs to the route above. Strategy: assign `sort_order = newIndex * 10` so collisions don't immediately cascade. We send only the moved card's new value — siblings keep theirs, gaps are fine.
- After a successful POST, JS does **not** re-render — the next full page load picks up the authoritative order. If the POST fails (non-2xx), the JS reverts the DOM by reloading the page.

For non-admins:

- The card is identical to today: no handles, no cursor change, no drag.

### Edge cases

1. **No envs / one env**: no drag affordance to set up. The empty-state and 1-card cases render normally.
2. **Two envs tied on `sort_order`**: tie-break is whatever the server returns; dragging either to a new position breaks the tie because the moved one gets `newIndex * 10`.
3. **Concurrent reorders**: two admins reordering at once race on `sort_order` values. Last write wins per env. We accept this — page reload reflects truth.
4. **Drag into same position** (no-op): skip the POST entirely. We don't want to write `sort_order` we already have.
5. **JS disabled**: drag silently doesn't work. There's no fallback up/down button in v1 (deliberately deferred — see non-goals).

---

## Verification Architecture

### Provable properties
None requiring formal proof; this is a thin trait-method + route + UI change.

### Test coverage

**Route tests** (`crates/forage-server/src/tests/platform_tests.rs`):

1. `reorder_environment_success_redirects` — admin POSTs valid form, gets 303 + Location to `/orgs/{org}/destinations`, mock receives the new `sort_order`.
2. `reorder_environment_invalid_csrf_returns_403`.
3. `reorder_environment_non_admin_returns_403` — member role.
4. `reorder_environment_non_member_returns_403`.
5. `reorder_environment_unauthenticated_redirects` — no session → 303 to `/login`.

**Mock**: extend `MockPlatformBehavior` with `update_environment_calls: Mutex<Vec<(String, i32)>>` so tests can assert what was called.

**Template**: the existing `destinations_page_returns_200` test already exercises render. We add `destinations_page_renders_drag_handles_for_admin` to assert the `draggable="true"` attribute appears for admins and is absent for non-admins. (Or assert on the `data-env-id` presence, which is a sufficient proxy.)

### Purity boundary

- Pure (`forage-core`): trait extension only — no I/O changes.
- Effectful (`forage-server`): gRPC call (forest_client.rs), route handler, template.
- Mock (`test_support.rs`): records calls.

---

## Implementation Notes

- Mirror the existing `create_environment` shape end-to-end — same imports, same `platform_authed_request`, same `map_platform_status`.
- The drag JS should be **vanilla HTML5 drag-and-drop** (no SortableJS) to keep deps zero. The template already includes inline `<script>` blocks; we add another one gated on `{% if is_admin %}`.
- Sort_order strides of 10 give plenty of headroom for inserts without renumbering the whole list.

---

## Out of scope / future work

- Renumber-on-collision background job.
- Drag-and-drop on touch devices (would need pointer events).
- A delete-environment affordance (separate concern, server endpoint exists).
