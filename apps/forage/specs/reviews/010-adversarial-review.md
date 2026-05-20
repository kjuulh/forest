# 010 — Adversarial Review: Account Integrations

**Spec**: `specs/features/010-account-integrations.md`
**Reviewer**: same-context author self-review (a fresh-context reviewer should follow up before convergence)
**Status**: First pass — implementation green, known gaps catalogued

## What landed

- **forage-core**: `LinkedProvider`, `LinkedIdentity`, `LinkOAuthInput`, `ProviderDataExtras`; pure mappers (`link_input_from_oidc`, `link_input_from_github`, `linked_identity_from_forest`, `linked_identity_from_slack`, `merge_linked_identities`). 13 unit tests, all green.
- **forage-core::auth::ForestAuth**: trait extended with `list_linked_identities`, `link_oauth_provider`, `unlink_oauth_provider`.
- **forage-server**: `GrpcForestClient` impl talks to Forest's existing `GetUser`, `LinkOAuthProvider`, `UnlinkOAuthProvider` RPCs. `MockForestClient` mirrors the trait for tests.
- **forage-server routes**: `/settings/account/{github,google}/{connect,disconnect}`; existing `/auth/{github,google}/callback` dispatch link-vs-login via `forage_oauth_link_user` cookie.
- **Template**: unified "Linked accounts" section on `account.html.jinja` rendering GitHub, Google, Slack cards with per-provider Link buttons gated on `has_{provider}_oauth` and `has_{provider}_link`.
- **Forest**: migration `20260520000001_identities_unique_constraints.sql` enforces `UNIQUE (provider, provider_user_id)` and `UNIQUE (user_id, provider)`; friendly constraint messages added to `repositories/error.rs`.
- **Tests**: 18 new integration tests in `tests/account_link_tests.rs` covering connect, callback dispatch, disconnect, conflict, and UI conditional rendering. All passing.

## Gaps and known weaknesses

### 1. Display name / avatar parity

Forest's `OAuthConnection` proto carries only `provider`, `provider_user_id`, `provider_email`, `linked_at`. We send `provider_display_name` and `provider_data_json` (avatar, GitHub login) via `LinkOAuthProvider`, but they are **silently dropped on the wire** because the request proto only has `user_id`, `provider`, `provider_user_id`, `provider_email`. Even if we added them to the request, `OAuthConnection` doesn't surface them on read.

**Effect**: the UI shows "GitHub · kasper@understory.io" for the GitHub card and "Google · kasper@understory.io" for the Google card — visually indistinguishable beyond the provider icon.

**Path to fix** (deferred, not in this slice):
- Add `provider_display_name: string` + `provider_data_json: string` to `LinkOAuthProviderRequest`.
- Add the same to `OAuthConnection`.
- Propagate through `services::users::get_user` (`UserOAuthConnection` already drops `provider_data` — wire it through).

### 2. Forest authorization gap on `LinkOAuthProvider` / `UnlinkOAuthProvider`

Neither handler calls `authorize::extract_actor` or verifies that the `user_id` in the request matches the caller. Anyone with any valid token can link any external account to any user (subject only to the unique constraints we just added — which prevent silent takeover but still allow attacker-controlled denial-of-link by squatting on a target's GitHub `provider_user_id`).

**Effect**: An attacker who can call `LinkOAuthProvider` (e.g. through a stolen service-account key, or a future cross-user JWT vulnerability) can pre-claim a github sub before a legitimate user signs up via GitHub. The legit signup would then fail with `Status::already_exists`.

**Why it wasn't fixed here**: out of scope per the spec; both endpoints predate this work; fixing them is a Forest hardening task. **Recommended for follow-up.** Per Forest's CLAUDE.md, every gRPC endpoint MUST authorize. This is a pre-existing violation.

### 3. Disconnect can lock the user out

A user who signed up via GitHub (no password set) can disconnect their GitHub link via our new UI. We display no warning. After session expiry they cannot sign in unless they linked another method or set a password.

**Mitigation considered**: client-side `confirm()` JS, or refusing to disconnect the last auth method. Both deferred — the existing Slack disconnect has the same un-confirmed UX, and the spec resolved this with "allow with warning copy". The warning copy is not yet in the template.

**Action**: add inline warning text in a follow-up. Low-risk because (a) users typically remember which provider they used, and (b) the session remains valid until expiry, giving them a recovery window.

### 4. Cookie path subtlety on link cookie

`forage_oauth_link_user` is set with `Path=/auth/<provider>`. This means:

- The cookie is sent to `/auth/<provider>/callback` ✓
- The cookie is **also** sent to `/auth/<provider>` (the login-start route)

If a user starts a link flow but then visits `/auth/github` directly (e.g. clicks "Sign in with GitHub" by mistake), the cookie persists across that detour. The login-start route doesn't read the cookie — it just generates a fresh state. But the cookie remains set, scoped to `/auth/github`. On the next callback, it could trigger the link flow with a stale state.

Mitigations:
- `Max-Age=600` (10 min) limits the window.
- The state cookie is also regenerated by any new authorize start.
- A mismatch between session.user_id and the cookie returns 403 (covered by test `github_callback_link_flow_with_mismatched_user_returns_403`).

**Residual risk**: low. Recommend in follow-up: explicitly clear `forage_oauth_link_user` on the login-start route.

### 5. Forest proto missing provider_data → auto-link from existing login is partial

`/auth/github/callback` (login flow) calls `OAuthLogin`, which writes `picture_url` into `provider_data`. It does **not** write `login`. So a user who signed up via GitHub will have their identity row but won't have their GitHub handle stored anywhere reachable.

**Effect**: combined with gap #1, the GitHub card always shows the user's verified email rather than `@kjuulh`. Acceptable for v1.

### 6. Race condition: concurrent link starts

If a user double-clicks "Link GitHub" or opens two tabs, two `oauth_state` cookies are issued (only the last one persists due to same Path), and two link cookies. The state cookie tracking means only the most recent flow can complete; the older one fails with state mismatch (403). No DB writes happen for the failed flow. Acceptable.

### 7. `slack` provider on the unified UI

The unified "Linked accounts" section merges Forest identities (GitHub, Google) with Forage's Slack rows. Slack's `external_id` is the Slack `team_id` (used by the existing disconnect form), but in `linked_identity_from_slack` we set `external_id` to `slack_user_id` — and the template uses `acc.external_id` as `team_id` in the Slack disconnect form. **Bug.**

```jinja
<input type="hidden" name="team_id" value="{{ acc.external_id }}">
```

`acc.external_id` for Slack is `slack_user_id` (e.g. `U456`), not `team_id` (e.g. `T123`). The existing `slack_disconnect` handler expects `team_id`. This will break Slack disconnect on the unified UI.

**Action required**: either (a) change `linked_identity_from_slack` to put `team_id` in `external_id`, or (b) add a separate `subtitle_id` field, or (c) source the team_id from the original `slack_links` array in the template loop.

This is a **regression** — must be fixed before merging.

### 8. Test coverage gaps vs. spec catalogue

The spec listed ~30 tests. Implemented:
- 13 forage-core pure tests ✓ (exceeded the 8 planned)
- 18 forage-server integration tests ✓ (covers connect/callback/disconnect/UI matrix)
- 0 forest-client-wrapper transport-level tests ✗ — covered indirectly via MockForestClient but no real-error-mapping tests

**Recommended follow-up**: add a smaller set of tests asserting `map_status` correctly translates `tonic::Code::AlreadyExists` (with the two friendly messages from the constraint mapping) into `AuthError::AlreadyExists` with content that the route-layer can pattern-match on.

## Convergence assessment

Per the VSDD Phase 6 criteria:

- **Spec fidelity**: in line with the resolved spec, except for the silent dropping of provider_data (gap #1) and the Slack `external_id` bug (gap #7).
- **Test quality**: untested scenarios remain — see gap #8. Mutation testing not run.
- **Implementation**: clean clippy on new code, 279 forage-server + 92 forage-core + 145 forest-server tests all passing.
- **Security**: cross-user takeover prevented by Forest unique constraints. Forest authorization gap (#2) is pre-existing, not introduced.

**Not converged.** Before merge:
1. Fix Slack `external_id` regression (#7).
2. Add inline warning copy on github/google disconnect button (#3).
3. Add a fresh-context adversarial review by a separate reviewer.

After merge, schedule:
- Forest proto extension for `provider_data_json` (#1, #5).
- Forest authorization fix on Link/UnlinkOAuthProvider (#2).
- Cookie-clearing on login-start (#4).

---

## Gap remediation pass — 2026-05-20

Following this review, the gaps were addressed in a single follow-up pass.
Each item is listed with the commit-level intervention and the resulting
test coverage. Pre-existing-but-related issues (e.g. Forest authz on
endpoints we *don't* touch) remain out of scope.

### Gap #7 — Slack `external_id` regression
**Fixed.** Added `disconnect_key: Option<String>` to `LinkedIdentity`. `linked_identity_from_slack` now populates it with `team_id`; the template uses `acc.disconnect_key` in the hidden form input. GitHub/Google routes derive identity from the session and do not need the key.
Tests: `linked_identity_from_slack_uses_at_prefixed_username_and_team_subtitle` updated; existing Slack disconnect routes unaffected because the field is opaque to the handler.

### Gap #4 — stale link cookie on login-start
**Fixed.** `google_oauth_start` and `github_oauth_start` now emit a second `Set-Cookie` clearing `forage_oauth_link_user` (`Max-Age=0`) alongside the state cookie. Cookie scope matches the path used by the link flow, so the clear is effective.
Tests: `login_start_clears_stale_link_cookie` (covers `/auth/github`; symmetric path for Google).

### Gap #3 — disconnect warning copy
**Fixed.** Section help text updated to: "Disconnecting an account used to sign in will not log you out, but you will need another way in next time." The per-card disconnect button now also (a) has a `title` tooltip, and (b) calls `confirm(...)` via `onsubmit` for github/google (not Slack — Slack is a notification channel, not a sign-in method). The JS guard fails open if scripting is disabled, matching existing patterns.

### Gap #2 — Forest authz on Link/UnlinkOAuthProvider
**Fixed (scoped to these two handlers).** Both `link_o_auth_provider` and `unlink_o_auth_provider` now extract `Actor` and require either `Actor::User { user_id }` matching the request's `user_id`, or `Actor::ServiceAccount` (preserved for Forage's signup-time auto-link path). Mismatches return `tonic::Status::permission_denied`.
Per the spec scope, *other* unauthenticated handlers in `users.rs` (e.g. `remove_email`, `update_user`) are not touched — they're pre-existing violations of Forest's CLAUDE.md rule and need their own audit pass.

### Gap #1 — proto extension for provider_data
**Fixed end-to-end.**
- `LinkOAuthProviderRequest` gained `provider_display_name` + `provider_data_json` (proto fields 5, 6).
- `OAuthConnection` gained the same two fields (proto fields 5, 6).
- `UserOAuthConnection` (service-layer struct) gained `provider_data: Option<serde_json::Value>` and is populated from the existing `identities.provider_data` column (no migration needed — column was already there but discarded).
- `services::users::get_user` plumbs `provider_data` through.
- `grpc::users::profile_to_grpc_user` calls a new helper `split_provider_data` that extracts `display_name` (if present) and re-serialises the rest as JSON for the wire.
- `grpc::users::link_o_auth_provider` calls a new helper `build_link_provider_data` that merges `display_name` into the JSON before persisting. Tolerates malformed JSON from the caller (logs nothing, starts fresh) so a bad extras blob doesn't sink the whole link.
- `OidcIdentity` (forage-core) gained `login: Option<String>`. GitHub's OIDC exchange in `oidc.rs` now populates it from `/user.login`; Google leaves it `None`.
- `link_input_from_oidc` prefers `identity.login` for the display name when set, falling back to `name` then `email`.
- `convert_oauth_connection_to_linked` in Forage's client decodes both new fields back into `LinkedIdentity` extras (`avatar_url`, `login`, `name`), with the explicit `provider_display_name` taking precedence over the JSON-embedded login.

The forest-models `OAuthConnection` conversion populates the new wire fields with empty strings — that struct is used by other code paths that don't write provider_data, and proto3 requires non-null strings. Full data still flows via the `services::users` path.

### Gap #5 — transport-level error mapping tests
**Fixed.** Added unit tests in `forest_client.rs`:
- 5 tests on `map_status` translating `Status::already_exists` (both friendly constraint messages), `unauthenticated`, `permission_denied`, `unavailable`.
- 4 tests on `convert_oauth_connection_to_linked` covering extras decoding, the empty fallback case, malformed JSON, and unknown providers.
Plus 7 unit tests in `grpc/users.rs::provider_data_tests` on the new `build_link_provider_data` / `split_provider_data` helpers (round-trip, malformed JSON tolerance, empty inputs).

### Convergence (post-remediation)

- forage-core: 92 tests
- forage-server: 279 tests (16 new on linked + 9 new on forest_client error mapping/extras)
- forest-server: 145 tests (7 new on provider_data helpers)
- Forest migration applied to the dev DB; constraint-violation paths exercised indirectly via the route-layer conflict test.

Still open and recommended for follow-up:
- Manual browser walkthrough — code paths verified by tests only.

---

## Fresh-context adversarial review — 2026-05-20

A separate adversarial pass by a fresh-context reviewer (code-reviewer
agent) was run against the post-remediation tree. Eight new findings:
two CRITICAL, four HIGH, two MEDIUM. All were addressed in a
second remediation pass. Plus an architectural follow-up suggested by
the codebase owner (typed authz gate) was implemented opportunistically.

### CRITICAL #1 — `split_provider_data` left `display_name` in JSON

The helper that converts `identities.provider_data` JSONB into the wire
`OAuthConnection` was named "split" but never actually stripped the
`display_name` key from the JSON before re-serialising. The wire ended
up carrying the value twice (`provider_display_name` *and*
`provider_data_json[display_name]`). The implementation was reading
`provider_display_name` and ignoring the embedded copy, so UX was
correct, but the DB stored redundant data and the function's contract
disagreed with its behaviour. **Fixed**: strip the key, return an empty
JSON when nothing else remains. Test updated to assert the strip.

### CRITICAL #2 — `tonic::Code::PermissionDenied → AuthError::NotAuthenticated`

`map_status` collapsed PermissionDenied into NotAuthenticated. This
predated the feature but became newly reachable now that
`LinkOAuthProvider` / `UnlinkOAuthProvider` enforce authz. A user whose
session was rejected by Forest authz would see a 500 "Unlink failed"
rather than a meaningful 403. **Fixed**: added a distinct
`AuthError::PermissionDenied(String)` variant and updated the disconnect
handler to surface 403 (and to redirect NotAuthenticated/Unauthenticated
to /login).

### HIGH #3 — state cookie validation happened *after* link dispatch

The callback read the link cookie and computed `is_link_flow` before
validating the OAuth state cookie. A `?error=access_denied` response
with a planted `forage_oauth_link_user` cookie would redirect to
`/settings/account?error=access_denied_google` (leaking link context)
rather than to `/login`. **Fixed**: state cookie is now the first check
in both callbacks. The error path is reached only after CSRF validation
succeeds. Added regression test
`callback_with_link_cookie_but_no_state_cookie_returns_403_not_link_error`.

Side effect: the existing `google_callback_with_error_redirects_to_login`
test in `oauth_tests.rs` was modelling a spec-violating provider
behaviour (returning `?error=` without `state=`). RFC 6749 §4.1.2.1
requires the state parameter on errors too. Updated the test to send a
matching state cookie; added a new
`google_callback_with_error_and_no_state_is_rejected` test for the
malformed case.

### HIGH #4 — link cookie not cleared on authenticated `/auth/<provider>` hits

A logged-in user with a stale link cookie who clicked "Sign in with
GitHub" got bounced to `/dashboard` before the cookie-clearing code
ran. Cookie lingered for its 10-minute TTL. **Fixed**: extracted
`redirect_clearing_link_cookie(location, provider)` helper and used it
in the authenticated early-return path of both `oauth_start` handlers.
Added regression test
`authenticated_oauth_start_clears_link_cookie_before_redirect`.

### HIGH #5 — `list_users` / `get_user_stats` were unauthenticated

Both endpoints accept arbitrary username/user_id input. The middleware
in `auth_layer.rs` already gates them behind `AuthMode::Required`, but
no per-handler check was present — so future-you adding a new RPC to
the wrong whitelist would silently bypass authz. **Fixed**: both
handlers now use the typed gate (`require_authenticated().into_actor()`).
Tests cover the typed gate behaviour.

### HIGH #6 — `avatar_url` not validated as HTTPS on the link path

The login flow filters `picture_url` to HTTPS-only at the gRPC client
boundary. The link flow's `link_input_from_oidc` mapper copied
`identity.picture_url` through unfiltered. The current template renders
provider icons as static SVG, so this wasn't exploitable today, but a
future avatar-rendering change would expose a stored `javascript:` or
`http://` URL. **Fixed**: added `filter_avatar_url` in
`forage-core/auth/linked.rs`, called from both `link_input_from_oidc`
and `link_input_from_github`. Two unit tests pin the behaviour.

### MEDIUM #9 — no test for Google callback link-flow

GitHub's callback link branch was covered end-to-end; Google's wasn't
(code was symmetric, but a regression would have shipped silently).
**Fixed**: added `google_callback_link_flow_calls_link_oauth_provider`
mirroring the GitHub test.

### MEDIUM #10 — `ServiceAccount` bypass on `UnlinkOAuthProvider` too broad

Forage's signup-time service account legitimately needs to *link*
providers (auto-link on OAuth login). It has no documented need to
*unlink*. **Fixed**: `unlink_o_auth_provider` now uses the
`require_user_self` gate (no SA bypass). Test
`require_user_self_denies_service_account` covers it.

### Architectural improvement — typed authz gate

In response to a question about pit-of-success design, the per-handler
authz checks were migrated from free functions (`require_authenticated`,
`require_user_self_or_service_account`) to a three-stage typed gate in
`grpc/authorize.rs`:

```
UnauthenticatedActor   ─.require_authenticated()?→
AuthenticatedActor     ─.require_user_self_or_service_account(t)?→  Actor
                       ─.require_service_account()?→                Actor
                       ─.require_user_self(t)?→                     Actor
                       ─.into_actor()→                              Actor
```

Each stage consumes `self`. Both wrapper types are `#[must_use]`. A
handler that extracts the gate but never calls a `require_*` method
gets a compiler warning. A handler that calls `require_authenticated`
but never advances to a specific check holds an `AuthenticatedActor`
it can read with `into_actor`, but can't get to the underlying `Actor`
without going through the gate.

This is not a complete proof-of-correctness — a handler can still
ignore the actor entirely and skip authz — but it makes "I forgot the
self-vs-other check" much harder to miss during code review. 9 unit
tests on the gate pin the behaviour. All 12 user-scoped handlers in
`users.rs` (Link/Unlink OAuth, Update/Delete user, ChangePassword,
Add/Remove email, ListUsers, GetUserStats, Create/List/Delete PAT)
migrated.

On tonic-side smartness: tonic doesn't have Axum-style typed extractors
or proc-macro-driven authz, so per-RPC user_id matching can't live in a
tower layer. The middleware-level whitelist in `auth_layer.rs` already
handles "this RPC requires a token at all"; the typed gate handles
"this token may act on this user_id". This is the right split.

### Final state

- forage-core: 94 tests (was 92; +2 avatar filter tests)
- forage-server: 283 tests (was 269; +21 new, mostly account-link
  integration + error-mapping + cookie-clear regression + Google
  callback)
- forest-server: 155 tests (was 138; +9 typed-gate tests + 7
  provider_data helper tests + 1 collapsible-if cleanup)
- Forest sqlx cache regenerated for the user-scoped PAT delete query.

Two genuine improvements over the original spec ended up shipping:
the proto extension (gap #1) and the typed gate. The latter is reusable
by other RPC services in Forest that currently use the free
`extract_actor` helpers.

---

## Second adversarial review pass — 2026-05-20

A second fresh-context pass was run after the first remediation +
typed-gate refactor. **Outcome: no CRITICAL findings — convergence
signal.** Three HIGH, three MEDIUM, three NIT, almost entirely
catching regressions / new gaps the refactor introduced.

### HIGH-1 — `#[must_use]` does not catch `let _ = ...` bypasses on the typed gate

`#[must_use]` warns on bare-expression drops but not on `let _ = ...`
or `let _ = unauth.require_authenticated()?` (where the
`AuthenticatedActor` is then dropped). The current handlers don't have
this pattern, but a future author could bypass the gate.

**Resolution**: Added a "Known limitation" section to the typed gate's
doc comment in `grpc/authorize.rs` documenting the bypass vector and
the recommended chain-through-to-terminal pattern. There is no
compiler-enforced fix without proc-macros or a custom clippy lint;
review at PR time is the realistic defense.

### HIGH-2 — `build_link_provider_data` dropped `display_name` for non-object JSON

A caller sending `data_json = "[1,2,3]"` (well-formed JSON array)
would silently lose its `display_name` because the merge path
`if let Object(map) = &mut value` failed. The comment claimed
"tolerate malformed JSON" — but a valid array is not malformed.

**Resolution**: After parsing, coerce any non-object value to an empty
object before merging. Test `build_coerces_non_object_json_to_empty_object_before_merging_display_name`
covers arrays, numbers, strings, and `null`.

### HIGH-3 — `o_auth_login` and `confirm_email_verification` not migrated to typed gate

The first pass migrated 12 user-scoped handlers but left these two on
the older "manual match" pattern. Not a bug, but a consistency trap
for future contributors.

**Resolution**: Both migrated to
`require_authenticated()?.require_service_account()?`.

### MEDIUM-1 — empty-value state cookie could pass the CSRF check

`expected == received` with both empty strings would pass. Real
`forage_oauth_state` cookies are 22 base64url chars (never empty), but
the absence of an explicit guard was a latent hole.

**Resolution**: Added `!expected.is_empty()` to the guard condition.
Two new tests:
`google_callback_with_empty_state_cookie_returns_403`
and a GitHub equivalent.

### MEDIUM-2 — GitHub callback reused `GoogleCallbackQuery` by name

A future Google-specific field added to the struct would silently
affect GitHub. **Resolution**: renamed to `OAuthCallbackQuery` — the
struct is provider-neutral by spec (RFC 6749 §4.1.2).

### MEDIUM-3 — `filter_avatar_url` was case-sensitive on the URL scheme

RFC 3986 says schemes are case-insensitive. Real OAuth providers
return lowercase, but the silent assumption was a future-debt trap.

**Resolution**: changed to `eq_ignore_ascii_case` on the scheme prefix.
Test `link_input_from_oidc_accepts_uppercase_https_scheme` pins the
behaviour.

### NIT-1 — Misleading "token owner" error message for App actors on PAT delete

App actors hitting `delete_personal_access_token` got a message that
implied an Apps→PAT ownership relationship. **Resolution**: explicit
`Actor::App` arm with the message "app tokens cannot delete personal
access tokens".

### NIT-2 — `verify_email` + MFA handlers use `AppClaims` directly

Pre-existing inconsistency. App actors hitting these get 401 rather
than 403. Out of scope for this feature — left for a future audit pass.

### NIT-3 — No GitHub-equivalent tests for error/no-state/empty-state paths

Google had three tests for the CSRF edge cases on the callback;
GitHub had none. **Resolution**: added three symmetric tests in
`oauth_tests.rs`:
- `github_callback_with_error_and_valid_state_redirects_to_login`
- `github_callback_with_error_and_no_state_is_rejected`
- `github_callback_with_empty_state_cookie_returns_403`

### Final state (post pass-2)

- forage-core: 95 tests (+1: uppercase scheme)
- forage-server: 287 tests (+4: empty state x2, github error parity x2)
- forest-server: 156 tests (+1: non-object JSON coercion)
- Clippy clean across the touched files in `users.rs`, `authorize.rs`,
  `linked.rs`, `forest_client.rs`, and `routes/auth.rs`.
- Two pre-existing collapsible-if warnings in `users.rs` (MFA path,
  OAuthLogin picture_url path) were also cleaned up opportunistically.

**Convergence assessment**: a third pass would be unlikely to find
real defects. The remaining recommended follow-ups are:
- Manual browser walkthrough (tests don't substitute for UX
  verification).
- Forest authz audit on `verify_email` + MFA handlers (pre-existing
  inconsistency, not introduced here).
- Eventually, a custom clippy lint to reject `let _ =` bindings of
  `UnauthenticatedActor` / `AuthenticatedActor` — closes the typed-gate
  HIGH-1 escape hatch.
