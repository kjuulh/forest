# 013 - OAuth Applications ("Sign in with Forest")

**Status**: Phase 5 — Implemented, reviewed, hardened. Complete except the deliberately-deferred `Actor::OAuthApp` general-API access (awaits API-access scopes beyond profile/email).

**Implemented so far** (forest-server `OAuthAppsService` + Forage `/oauth/*`):
- M1 App registration CRUD + Developer Settings UI (org admin).
- M2 Authorization-code flow: `/oauth/authorize` consent + `/oauth/token` (authorization_code), single-use codes, exact redirect match, **PKCE** (S256/plain), open-redirect defence.
- M3 `/oauth/userinfo` with scope-gated claims (`profile`/`email`).
- M4 Refresh-token grant (rotation + reuse → family revoke), `RevokeOAuthGrant`.
- Authorized-apps account page (`ListOAuthGrants` + revoke).

**Remaining (hardening)**: `Actor::OAuthApp` + per-RPC scope enforcement for general API access (deferred until API-access scopes exist beyond profile/email); real OAuth-client-library E2E; property tests; adversarial review; clippy/cargo-audit.

**Original status**: Phase 1 — Spec Crystallisation
**Depends on**: 002 (Authentication), 003 (BFF Sessions), 010 (Account Integrations)
**Related**: forest-server `UsersService` (OAuthLogin, PATs, device login), `personal_access_tokens` / `sessions` token model, forest `auth_layer` token resolution.

## Problem

Third parties cannot integrate with Forest on a user's behalf. There is no way for an
external application to say "let this user sign in with their Forest account" or "let me
call the Forest API as this user, with their consent, limited to specific scopes".

Today the only programmatic credentials are:

- **Personal access tokens** — minted by a user for themselves; no third-party consent, no
  client identity, no scoped delegation.
- **The device-login flow** — first-party (the Forest CLI), gated behind Forage's
  service-account credential; not available to arbitrary third parties.

We want an organisation to be able to register an **OAuth Application** — the equivalent of a
GitHub OAuth App — under **Developer Settings** in org settings. The org configures a name,
homepage, callback (redirect) URLs and the scopes the app may request. Forest issues a
`client_id` and a `client_secret`. A third party then runs the standard **OAuth 2.0
authorization-code flow** against Forest to obtain an access token (and refresh token) for a
consenting Forest user, and reads that user's profile/email from a `userinfo` endpoint —
exactly the shape of "Sign in with GitHub".

## Separation of Concerns

This follows the established Forest/Forage split (cf. specs 006, 010): **Forest owns the
data and protocol; Forage owns the UI and the public HTTP surface.**

**Forest** (upstream gRPC server) — **source of truth**:
- Owns OAuth applications (client_id, hashed client_secret, redirect URIs, allowed scopes,
  owning org), authorization codes, and the access/refresh tokens issued to those apps.
- Owns the authorization-server *logic*: validating a client + redirect_uri + scopes,
  minting single-use authorization codes after a user consents, exchanging codes for tokens,
  refreshing tokens, revoking, and resolving an access token to user claims (userinfo).
- Extends the existing `UsersService` token model — OAuth-app access tokens are first-class
  bearer credentials, resolvable by the existing `auth_layer` (alongside PATs and app tokens).
- Enforces org admin/owner authorization on all app-management RPCs.

**Forage** (this codebase — the BFF):
- Renders **Developer Settings** under org settings: list / create / view / edit / delete
  OAuth apps, show the `client_secret` once, rotate the secret. Owner/admin only.
- Hosts the **public HTTP OAuth endpoints** (`/oauth/authorize`, `/oauth/token`,
  `/oauth/userinfo`), delegating all validation/issuance to Forest over gRPC.
- Renders the **consent screen** at `/oauth/authorize` using the existing browser session as
  the resource owner (the logged-in Forest user), exactly as `/device` does for device login.
- Stores **no** OAuth-app state of its own.

## Scope

Covered:
- **OAuth app model** in Forest: CRUD RPCs, owning org, redirect URIs, allowed scopes.
- **Authorization-code grant** (RFC 6749 §4.1) with **PKCE** (RFC 7636) support.
- **Token endpoint**: `authorization_code` and `refresh_token` grants; opaque tokens.
- **Userinfo endpoint**: returns claims gated by granted scopes.
- **Scopes**: `profile` (username, user_id, avatar) and `email` (verified emails). Fixed catalog.
- **Consent screen** in Forage, reusing the session + CSRF patterns.
- **Developer Settings** UI in Forage org settings (sidebar entry, list, create, detail, delete).
- **Token revocation** per app + per user (revoke a user's grant; revoke/rotate app secret).

Out of scope (model supports later):
- **OIDC core implemented** (post-MVP): `openid` scope, `id_token` (HS256 JWT signed with the
  client_secret) on code exchange + refresh, `nonce` echoed into the id_token, `prompt=none`
  (silent auth / `consent_required`) and `prompt=consent` (force re-consent), and
  `/.well-known/openid-configuration` discovery. Still out: RS256 + JWKS (asymmetric
  verification) and the full standard claims set.
- Additional scopes (`read:org`, write scopes, per-project scopes).
- Client-credentials and device grants for third-party apps.
- Per-app rate limiting and a developer-facing usage dashboard.
- App logos / verified-publisher badges.

## Architecture

### Roles (OAuth 2.0)

- **Resource owner**: a logged-in Forest user (the Forage browser session).
- **Client**: the registered OAuth app (`client_id` + `client_secret`), owned by an org.
- **Authorization server**: Forest (logic) fronted by Forage's HTTP endpoints.
- **Resource server**: Forest's gRPC API (access tokens resolve via `auth_layer`).

### OAuth application model (Forest)

```
oauth_app:
  id                  UUID
  organisation_id     UUID        -- owning org (FK organisations)
  name                TEXT        -- display name, shown on consent screen
  description         TEXT
  homepage_url        TEXT
  client_id           TEXT UNIQUE -- public, e.g. "forest_oauth_<random>"
  client_secret_hash  BYTEA       -- SHA-256 of secret; plaintext shown once
  redirect_uris       TEXT[]      -- exact-match allowlist
  scopes              TEXT[]      -- scopes this app is permitted to request
  created_by          UUID
  created_at, updated_at
```

Authorization codes and issued tokens (Forest):

```
oauth_authorization_code:
  code_hash    BYTEA PRIMARY KEY  -- SHA-256(raw code); raw never stored
  app_id       UUID (FK oauth_app ON DELETE CASCADE)
  user_id      UUID (FK users)
  redirect_uri TEXT               -- must match at exchange
  scopes       TEXT[]             -- consented scopes
  code_challenge        TEXT      -- PKCE, nullable
  code_challenge_method TEXT      -- "S256" | "plain" | NULL
  expires_at   TIMESTAMPTZ        -- short (e.g. 60s); single-use
  consumed_at  TIMESTAMPTZ

oauth_access_token:
  token_hash    BYTEA PRIMARY KEY -- SHA-256(raw access token)
  app_id        UUID (FK oauth_app ON DELETE CASCADE)
  user_id       UUID (FK users)
  scopes        TEXT[]
  refresh_hash  BYTEA UNIQUE      -- SHA-256(raw refresh token), nullable
  expires_at    TIMESTAMPTZ       -- access token TTL (e.g. 8h)
  refresh_expires_at TIMESTAMPTZ  -- refresh TTL (e.g. 90d)
  revoked_at    TIMESTAMPTZ
  created_at, last_used_at
```

Tokens reuse the established pattern: a random raw token (base64url / hex) returned to the
client once, only its SHA-256 hash stored. Resolution mirrors `resolve_personal_access_token`
in `auth_layer` — an OAuth access token resolves to `Actor::OAuthApp { user_id, app_id, scopes }`
(new actor variant) so downstream RPCs can later honour scopes.

### Scope catalog

| scope     | userinfo claims unlocked                                   |
|-----------|------------------------------------------------------------|
| `profile` | `sub` (user_id), `username`, `profile_picture_url`         |
| `email`   | `email` (primary verified), `emails[]` (all verified)      |

`sub` is always present. Unknown/unpermitted scopes are rejected at `/authorize`.

### gRPC interface (Forest — new `OAuthAppService` or additions to `UsersService`)

App management (org admin/owner gated):
- `CreateOAuthApp(org_id, name, description, homepage_url, redirect_uris[], scopes[])`
  → app + raw `client_secret` (once).
- `ListOAuthApps(org_id)` → apps (no secrets).
- `GetOAuthApp(org_id, app_id)` → app (no secret).
- `UpdateOAuthApp(org_id, app_id, …)` → app.
- `RotateOAuthAppSecret(org_id, app_id)` → new raw `client_secret` (once).
- `DeleteOAuthApp(org_id, app_id)` → cascades codes/tokens.

Authorization-server (service-account gated — only Forage calls these):
- `LookupOAuthClient(client_id)` → public app metadata for rendering `/authorize`
  (name, org, redirect_uris, scopes) — no secret.
- `CreateOAuthAuthorizationCode(client_id, user_id, redirect_uri, scopes[], code_challenge,
  code_challenge_method)` → raw single-use code. (Called after the user consents.)
- `ExchangeOAuthCode(client_id, client_secret, code, redirect_uri, code_verifier)`
  → `AuthTokens` (access + refresh + expires_in) + granted scopes.
- `RefreshOAuthToken(client_id, client_secret, refresh_token)` → new `AuthTokens`.
- `GetOAuthUserinfo(access_token)` → claims filtered by the token's scopes.
- `RevokeOAuthGrant(user_id, app_id)` — user-self gated — drops all tokens for that app.

### Public HTTP endpoints (Forage)

- `GET  /oauth/authorize` — params `client_id`, `redirect_uri`, `response_type=code`,
  `scope`, `state`, optional `code_challenge`, `code_challenge_method`. Requires a logged-in
  session (else redirect to `/login?return_to=…`). Calls `LookupOAuthClient`, validates
  `redirect_uri` is in the allowlist and requested scopes ⊆ app scopes, then renders the
  consent screen. **On invalid `client_id`/`redirect_uri`, render an error page — never
  redirect** (open-redirect / mix-up defence). Other errors redirect to `redirect_uri` with
  `?error=…&state=…` per RFC 6749 §4.1.2.1.
- `POST /oauth/authorize` — consent decision (CSRF-protected). Approve → `CreateOAuthAuthorizationCode`
  → 302 to `redirect_uri?code=…&state=…`. Deny → 302 with `?error=access_denied&state=…`.
- `POST /oauth/token` — form-encoded; `grant_type` ∈ {`authorization_code`,`refresh_token`}.
  Client auth via `client_secret_post` (body) or HTTP Basic. Returns JSON
  `{access_token, token_type:"bearer", expires_in, refresh_token, scope}` or an OAuth error JSON.
- `GET  /oauth/userinfo` — `Authorization: Bearer <token>` → JSON claims. 401 on invalid token.

### Forage Developer Settings (UI)

Routes under the existing `/orgs/{org}/settings/*` pattern, owner/admin gated via
`require_org_membership` + `require_admin`, CSRF on all POSTs (matching `members`/`integrations`):

```
GET  /orgs/{org}/settings/developers                       -- list apps
GET  /orgs/{org}/settings/developers/new                   -- create form
POST /orgs/{org}/settings/developers                       -- create (secret shown once)
GET  /orgs/{org}/settings/developers/{app_id}              -- detail / edit
POST /orgs/{org}/settings/developers/{app_id}              -- update
POST /orgs/{org}/settings/developers/{app_id}/rotate       -- rotate secret (shown once)
POST /orgs/{org}/settings/developers/{app_id}/delete       -- delete
```

Sidebar gains a **Developer Settings** entry (`settings_sidebar.html.jinja`,
`active_section == "developers"`). New templates: `pages/developers.html.jinja` (list +
create), `pages/developer_app.html.jinja` (detail/edit/secret), `pages/oauth_consent.html.jinja`
(the consent screen).

## Behavioural Contract

### Authorize (`GET /oauth/authorize`)
- **Pre**: user authenticated; `client_id` exists; `redirect_uri` ∈ app allowlist;
  `response_type == "code"`; requested scopes ⊆ app scopes; if `code_challenge` present,
  `code_challenge_method` ∈ {`S256`,`plain`}.
- **Post**: consent screen rendered listing the app, org, and human-readable scope descriptions.
- **Invariants**: unknown `client_id` or non-allowlisted `redirect_uri` ⇒ on-site error page,
  **no redirect**. Other invalid params ⇒ redirect to `redirect_uri` with `error`+`state`.

### Consent (`POST /oauth/authorize`)
- **Pre**: valid CSRF; same validations as authorize re-checked server-side.
- **Post (approve)**: a single-use code (TTL ≤ 60s) bound to (app, user, redirect_uri, scopes,
  PKCE challenge) is created; 302 to `redirect_uri?code&state`.
- **Post (deny)**: 302 to `redirect_uri?error=access_denied&state`.

### Token — authorization_code (`POST /oauth/token`)
- **Pre**: valid `client_id`+`client_secret`; `code` exists, unconsumed, unexpired, belongs to
  this client; `redirect_uri` matches the one bound to the code; if a `code_challenge` was
  stored, `code_verifier` validates (S256/plain).
- **Post**: code marked consumed (atomic); access+refresh tokens minted, hashes stored;
  JSON token response returned.
- **Invariants**: a code is redeemable at most once (replay ⇒ `invalid_grant` *and* prior
  tokens for that code revoked); client mismatch ⇒ `invalid_grant`; bad secret ⇒
  `invalid_client` (401).

### Token — refresh_token
- **Pre**: valid client creds; refresh token exists, unexpired, unrevoked, belongs to client.
- **Post**: new access token (and rotated refresh token); old refresh hash invalidated.

### Userinfo (`GET /oauth/userinfo`)
- **Pre**: bearer access token valid, unexpired, unrevoked.
- **Post**: JSON of `sub` plus claims permitted by the token's scopes; `last_used_at` touched.
- **Invariants**: a claim never appears without its scope; expired/revoked ⇒ 401.

### App management
- All RPCs/routes require the caller be owner/admin of the owning org.
- `client_secret` (create) and rotated secret are returned exactly once; only the hash persists.
- `redirect_uris` validated: absolute `https://` URLs (or `http://localhost[:port]` /
  `http://127.0.0.1[:port]` for dev), no fragment; at least one required.
- Delete cascades authorization codes and tokens.

## Edge Cases
- Requested scope not in app's allowlist → authorize error `invalid_scope`.
- `redirect_uri` differs by even a trailing slash → treated as non-match (exact match).
- Authorization code reuse → `invalid_grant` + revoke tokens derived from it (RFC 6749 §4.1.2).
- Refresh-token reuse after rotation → `invalid_grant`; revoke the token family.
- Org deleted / app deleted mid-flow → outstanding codes/tokens cease to resolve.
- User revokes a grant → that app's tokens stop resolving immediately.
- PKCE `code_verifier` mismatch → `invalid_grant`.
- Two tabs / concurrent code exchange → code consumption is atomic (`UPDATE … WHERE consumed_at IS NULL`).
- Open-redirect: only on-site error pages for invalid client/redirect; never reflect to an
  unvalidated URI.
- `client_secret` comparison is constant-time (compare hashes).

## Non-Functional Requirements
- **Security**: secrets/codes/tokens stored only as SHA-256 hashes; constant-time secret
  compare; codes single-use and short-lived; PKCE supported; exact redirect-uri match; consent
  required every authorization (MVP — no silent re-consent); CSRF on consent POST; HTTPS-only
  redirect URIs (localhost exempt).
- **Performance**: token + userinfo are single-hash-lookup gRPC calls; no N+1.
- **Compatibility**: token endpoint conforms to RFC 6749 error JSON; works with standard OAuth
  client libraries configured with Forest's authorize/token/userinfo URLs.

## Verification Architecture
- **Pure core (testable without DB/network)**: scope parsing/validation & subset checks;
  redirect-uri allowlist matching; PKCE S256/plain verification; OAuth error→response mapping;
  consent-screen view-model construction; client_id/secret/token generators (format/entropy).
  These live in `forage-core` (HTTP-side helpers) and Forest domain modules (issuance/validation).
- **Effectful shell**: Forage HTTP handlers, Forest gRPC handlers, repositories, event store.
- **Property tests**: (1) a code validates at most once; (2) userinfo never emits a claim whose
  scope was not granted; (3) redirect-uri match is exact (no prefix/suffix bypass); (4) PKCE:
  only the matching verifier validates; (5) generated secrets/tokens are unguessable (length/charset).

## Open Questions (resolved)
- Protocol: **plain OAuth2 + userinfo**, OIDC-ready (structured for later `id_token`).
- Ownership: **Forest backend** owns apps/codes/tokens; **Forage** owns UI + HTTP endpoints.
- Token model: **access + refresh**, opaque, hashed; consent + per-grant revocation.
- Scopes (MVP): **`profile` + `email`**.
