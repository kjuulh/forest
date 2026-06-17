# 013 — Adversarial Review: OAuth Applications ("Sign in with Forest")

**Spec**: `specs/features/013-oauth-applications.md`
**Reviewer**: same-context author self-review (a fresh-context reviewer should follow up before convergence)
**Status**: First pass — implementation green; one real bug found & fixed, remaining gaps catalogued

## What landed

- **forest-server `OAuthAppsService`**: app CRUD (org-admin gated) + authorization-server RPCs
  (`LookupOAuthClient`, `CreateOAuthAuthorizationCode`, `ExchangeOAuthCode`, `GetOAuthUserinfo`,
  `RefreshOAuthToken`, `RevokeOAuthGrant`, `ListOAuthGrants`) — service-account gated.
- **Tables**: `oauth_apps`, `oauth_authorization_codes`, `oauth_access_tokens` (hashes only).
- **Forage**: Developer Settings UI; public `/oauth/{authorize,token,userinfo}`; consent screen;
  authorized-apps account page.
- **Tests**: 25 forest unit + property tests, 9 forest E2E accept tests, ~25 Forage route tests.

## Findings

### FIXED — Refresh-token rotation was not single-use under concurrency (TOCTOU)

`refresh_token` originally did `find_token_by_refresh` (read) → `revoke_token_by_refresh` (write)
as two separate statements. Two concurrent refreshes presenting the same refresh token both passed
the read, both revoked (idempotently), and **both minted a new access+refresh pair** — a refresh
token usable more than once.

**Fix**: replaced with an atomic conditional consume
(`UPDATE … SET revoked_at = now() WHERE refresh_hash = $1 AND app_id = $2 AND revoked_at IS NULL …
RETURNING …`). Only one concurrent caller's UPDATE can match, so rotation is genuinely single-use.
If the consume matches nothing, a follow-up lookup distinguishes *reuse of a rotated token*
(→ revoke the whole `(user, app)` family, RFC 6749 §10.4) from *unknown/expired* (→ `invalid_grant`).
Mirrors the authorization-code `consume_authorization_code` pattern.

**Regression test**: `accepttest::oauth_flow::concurrent_refresh_only_one_succeeds` fires two
simultaneous refreshes with the same token and asserts exactly one succeeds.

### Verified-good (attempted to break, held up)

- **Single-use authorization codes**: `consume_authorization_code` is an atomic conditional UPDATE;
  replay → `invalid_grant`. Code consumption happens *after* client-secret verification, so a wrong
  secret doesn't burn a code.
- **Code binding**: exchange rejects unless the code's `app_id` matches the presenting client *and*
  the `redirect_uri` matches exactly.
- **Open-redirect / mix-up**: unknown `client_id` or non-allowlisted `redirect_uri` at `/authorize`
  render an on-site error — never a redirect. Validated server-side again on the consent POST
  (hidden `redirect_uri` is not trusted).
- **Exact redirect match**: `==` over the allowlist; property test confirms no scheme/host bypass.
- **PKCE**: S256 + plain; property test confirms only the matching verifier validates.
- **Client enumeration**: `UnknownClient` and `InvalidClientSecret` both map to
  `Unauthenticated("invalid_client")` — same code + message.
- **Constant-time secret compare**: `constant_time_eq` over SHA-256 hashes (property-tested).
- **Scope gating**: userinfo emits a claim only if its scope was granted (E2E test withholds email
  when only `profile` is granted). Requested scopes must be ⊆ the app's allowlist.
- **Storage**: secrets/codes/access/refresh stored only as SHA-256 hashes; raw shown once.

## Remaining gaps (catalogued, not blocking the MVP)

### 1. Token endpoint client auth — RESOLVED

~~Token endpoint supported `client_secret_post` only, not HTTP Basic.~~ The token endpoint now
accepts credentials via **HTTP Basic** (RFC 6749 §2.3.1, with form-urlencoded component decoding)
*and* `client_secret_post`, with Basic taking precedence. `parse_basic_auth` has 5 unit tests
(plain, url-decoded components, empty secret, non-Basic/missing, malformed) plus an integration
test (`token_accepts_http_basic_client_auth`).

### 2. Consent integrity under hidden-field tampering — RESOLVED

~~A tampered hidden `scope` field could grant a different (in-allowlist) scope set than displayed.~~
The rendered consent params (`client_id`, `redirect_uri`, `scope`) now carry a
`consent_binding` = `HMAC-SHA256(session.csrf_token, client_id‖redirect_uri‖scope)`. The consent
POST recomputes the tag from the submitted fields and the session's secret csrf_token and rejects
(403) on mismatch (constant-time compare), so the displayed params can't be altered between render
and submit. Test: `consent_with_tampered_scope_is_rejected` (binding bound to `profile`, POST
widened to `profile email` → 403).

### 3. Remembered consent — RESOLVED

~~Every authorization showed the consent screen; a returning user re-approved each time.~~
Added an `oauth_consents` table (PK `(app_id, user_id)`, scopes accumulate). Approving records
consent (`upsert_consent` at code creation); `GetOAuthConsent` exposes it; `GET /oauth/authorize`
auto-approves (issues a code without the screen) when the requested scopes ⊆ prior consent, and
still prompts when a new scope is requested; `RevokeOAuthGrant` clears it so the next authorization
re-prompts. Tests: forest `consent_is_remembered_and_cleared_on_revoke` (record → union → cleared);
Forage `authorize_auto_approves_when_prior_consent_covers_scopes` +
`authorize_prompts_when_prior_consent_is_insufficient`.

### 4. `Actor::OAuthApp` (general-API access) intentionally absent

OAuth access tokens resolve only through the scope-gated `GetOAuthUserinfo` RPC, **not** the general
`auth_layer`. This is deliberate: the MVP scopes (`profile`, `email`) map to identity, not API
operations, so wiring tokens into `auth_layer` as a plain `Actor::User` would grant full,
*unscoped* user power — a privilege-escalation hole. Deferred until API-access scopes
(`read:projects`, …) and per-RPC scope enforcement exist.

### 5. Real-HTTP client E2E — PARTIALLY RESOLVED

Added `oauth_e2e_tests::real_http_client_completes_authorization_code_flow`: binds the real Forage
router to a TCP port and drives consent → token → userinfo with `reqwest` (real transport,
form-encoding, 303 handling, JSON parsing, bearer auth, and a 401 + `WWW-Authenticate` on a missing
token). The forest-server side is still mocked (its gRPC wire is covered by forest's accept tests),
so the one seam not exercised by an automated test is the live two-process Forage↔forest gRPC link
under a real deployment — recommended as a manual/CI smoke check.

### 6. Housekeeping / reaping — RESOLVED

~~Expired codes/tokens were filtered at read time but never deleted.~~ Added `OAuthReaper`
(a `notmad::Component` wired into `cli/serve.rs` alongside `ReleaseReaper`, hourly) backed by
`OAuthAppRepository::reap_expired`, which prunes expired/consumed authorization codes and
fully-dead access tokens (refresh — or access, when there is no refresh — expired) plus tokens
revoked more than 7 days ago. Recently-revoked tokens are deliberately kept so refresh-reuse
detection still has a signal. Accept test `reaper_prunes_dead_rows_but_keeps_live_token` verifies
dead rows are removed while a live token still resolves via userinfo (3× idempotent).
