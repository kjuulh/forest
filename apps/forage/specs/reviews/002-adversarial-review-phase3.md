# Adversarial Review 002 - Post Spec 004 (Projects & Usage)

**Date**: 2026-03-07
**Scope**: Full codebase review after specs 001-004
**Tests**: 53 total (17 core + 36 server), clippy clean
**Verified**: Against real forest-server on localhost:4040

---

## 1. Architecture: Repeated gRPC Calls Per Page Load

**Severity: High**

Every authenticated platform page (`projects_list`, `project_detail`, `usage`) calls `list_my_organisations` to verify membership. This means:

- `/orgs/testorg/projects` -> 1 call to list orgs + 1 call to list projects = **2 gRPC calls**
- `/orgs/testorg/projects/my-api` -> 1 call to list orgs + 1 call to list artifacts = **2 gRPC calls**
- Dashboard -> 1 call to list orgs (redirect) then the target page makes its own calls

This is the same pattern we fixed for `get_user()` in spec 003 (caching user in session). The org list should be cached in the session too, or at minimum passed through from the `Session` extractor.

**Recommendation**: Cache the user's org memberships in `SessionData` / `CachedUser`. Refresh on session refresh or after a configurable TTL. This eliminates the most expensive repeated call.

---

## 2. Architecture: Two Traits, One Struct, Inconsistent Error Handling

**Severity: Medium**

`GrpcForestClient` now implements both `ForestAuth` and `ForestPlatform`. The `authed_request` helper is duplicated:
- `GrpcForestClient::authed_request()` returns `AuthError`
- `platform_authed_request()` is a free function returning `PlatformError`

Same logic, two copies, two error types. `AppState` holds `Arc<dyn ForestAuth>` + `Arc<dyn ForestPlatform>` which in production point to the same struct. This is fine for testability but means the constructors are getting wide (4 args now).

**Recommendation**: Consider a single `ForestClient` trait that combines both, or unify the auth helper into a generic form. Not urgent but will become pain as more services are added.

---

## 3. Security: Org Name in URL Path is User-Controlled

**Severity: Medium**

Routes use `{org}` from the URL path and pass it directly to gRPC calls and template rendering:
- `format!("{org} - Projects - Forage")` in HTML title
- `format!("Projects in {org}")` in meta description

MiniJinja auto-escapes by default in HTML context, so XSS via `<script>` in org name is mitigated. However:
- The `title` tag is outside normal HTML body escaping in some edge cases
- The `description` meta tag uses attribute context escaping

**Recommendation**: Validate or sanitize `{org}` and `{project}` path params at the route level. The org membership check already prevents arbitrary names from rendering (403 if not a member), but defense in depth matters.

---

## 4. Session: `last_seen_at` Updated on Every Request

**Severity: Low**

The `Session` extractor calls `state.sessions.update()` on **every single request** to update `last_seen_at`. For the PostgreSQL store, this means a write query per page load. For the in-memory store, it's a write lock on the HashMap.

**Recommendation**: Only update `last_seen_at` if the previous value is older than some threshold (e.g., 5 minutes). This is a simple check that eliminates 95%+ of session writes.

---

## 5. Testing: No Test for the `ForestPlatform` gRPC Implementation

**Severity: Medium**

The `GrpcForestClient` `ForestPlatform` impl (lines 294-393 of `forest_client.rs`) has zero test coverage. It's only tested indirectly via integration tests that use `MockPlatformClient`. The mapping from proto types to domain types (`Organisation`, `Artifact`) is untested.

Specifically:
- The `zip(resp.roles)` could silently truncate if lengths don't match
- The `unwrap_or_default()` on `a.context` hides missing data
- The empty-string-to-None conversion for `description` is a subtle behavior

**Recommendation**: Add unit tests for the proto-to-domain conversion functions. Extract them into named functions (like `convert_user` and `convert_token` for auth) to make them testable.

---

## 6. Testing: Dashboard Test Changed Behavior Without Full Coverage

**Severity: Medium**

`dashboard_with_session_returns_200` was renamed to `dashboard_with_session_redirects_to_org` and now only checks for `StatusCode::SEE_OTHER`. The old test verified the dashboard rendered with `testuser` content. The new behavior (redirect) is tested, but the onboarding page content is only tested in `dashboard_no_orgs_shows_onboarding` which checks for `"forest orgs create"`.

Nobody tests:
- What happens if `list_my_organisations` returns an error (not empty, an actual error)
- The dashboard template rendering is correct (title, user info)

**Recommendation**: Add test for platform unavailable during dashboard load.

---

## 7. VSDD Process: Spec 004 Skipped the Red Gate

**Severity: Medium (Process)**

The VSDD spec says "All tests must fail before implementation begins." In spec 004, we wrote templates, routes, AND tests in the same step. Tests never had a Red phase - they were green on first run. This is pragmatic but violates VSDD.

The earlier specs (001-003) had proper Red->Green cycles. Spec 004 was implemented as "write everything at once."

**Recommendation**: For future specs, write the test assertions first with stub routes that return 501/500, verify they fail, then implement. Even if the cycle is fast, the discipline catches assumption errors.

---

## 8. Template: No Authenticated Navigation

**Severity: Medium (UX)**

The spec called for "Authenticated navigation with org switcher" but it wasn't implemented. All pages (projects, usage, onboarding) use the same marketing `base.html.jinja` which shows "Pricing / Components / Sign in" in the nav, even when the user is authenticated and browsing their org's projects.

This means:
- No way to switch orgs from the nav
- No visual indication you're logged in (except the page content)
- No link back to projects/usage from the nav on authenticated pages

**Recommendation**: Either pass `user` and `orgs` to the base template and conditionally render an app nav, or create a separate `app_base.html.jinja` that authenticated pages extend.

---

## 9. Error UX: Raw Status Codes as Responses

**Severity: Medium**

403 and 500 errors return bare Axum status codes with no HTML body:
- Non-member accessing `/orgs/someorg/projects` -> blank 403 page
- Template error -> blank 500 page

**Recommendation**: Add simple error templates (`403.html.jinja`, `500.html.jinja`) and render them instead of bare status codes. Even a one-line "You don't have access to this organisation" is better than a browser default error page.

---

## 10. Code: `expires_in_seconds` is Suspiciously Large

**Severity: Low (Upstream)**

During integration testing, forest-server returned `expiresInSeconds: 1775498883` which is ~56 years. This is likely a bug in forest-server (perhaps it's returning an absolute timestamp instead of a duration). Our code treats it as a duration: `now + Duration::seconds(tokens.expires_in_seconds)`. If forest-server is actually returning a Unix timestamp, we'd set expiry to year 2082.

The session refresh logic would never trigger, which means tokens are effectively permanent. The BFF session protects the browser from this (sessions expire by `last_seen_at` reaper), but the underlying token is never refreshed.

**Recommendation**: Verify with forest-server what `expires_in_seconds` actually means. If it's a bug, cap it to a sane maximum (e.g., 24h) client-side.

---

## 11. Missing: CSRF Protection on State-Mutating Endpoints

**Severity: Medium (Security)**

`POST /logout`, `POST /login`, `POST /signup`, `POST /settings/tokens`, `POST /settings/tokens/{id}/delete` all accept form submissions with no CSRF token. The `SameSite=Lax` cookie provides baseline protection against cross-origin POST from foreign sites, but:

- `SameSite=Lax` allows top-level navigations (e.g., form auto-submit from a link)
- A CSRF token is the standard defense-in-depth

**Recommendation**: Add CSRF tokens to all forms. MiniJinja can render a hidden `<input>` and the server validates it against a session-bound value.

---

## Prioritized Actions

### Must Do (before next feature)
1. **Error pages**: Add 403/500 error templates (bare status codes are bad UX)
2. **Authenticated nav**: Implement app navigation for logged-in users
3. **Platform-unavailable test**: Add test for dashboard when `list_my_organisations` errors

### Should Do (this iteration)
4. **Cache org memberships in session**: Eliminate repeated `list_my_organisations` gRPC call
5. **Throttle session writes**: Only update `last_seen_at` if stale (>5min)
6. **Extract proto conversion functions**: Make them testable, add unit tests
7. **CSRF tokens**: Add to all POST forms

### Do When Relevant
8. **Unify auth helper**: Deduplicate `authed_request` / `platform_authed_request`
9. **Validate path params**: Sanitize `{org}` and `{project}` at route level
10. **Investigate `expires_in_seconds`**: Confirm forest-server semantics, cap if needed
11. **VSDD discipline**: Enforce Red Gate for future specs
