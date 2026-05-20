# 010 - Account Integrations (GitHub & Google linking)

**Status**: Phase 1 — Spec Crystallisation
**Depends on**: 002 (Authentication), 006 (Notification Integrations)
**Related**: existing Slack user-link flow in `auth.rs` (`/settings/account/slack/{connect,callback,disconnect}`)

## Problem

Forage already lets a user log in via Google, GitHub, or Slack (`/auth/google`, `/auth/github`) and lets them **link a Slack identity** post-login on the account page (`/settings/account/slack/connect`). The Slack link records the workspace + Slack username so future personal DMs can be addressed to the right `@user`.

GitHub and Google are wired only for login — there is **no symmetric "link my GitHub" / "link my Google" affordance** on the account page. As a result:

1. **No GitHub identity on file** post-login. We cannot map a release's `commit_author = "kjuulh"` to a Forage user, even though the user may have signed in with GitHub. The OAuth login currently extracts the GitHub identity but only uses it to mint the session; the identity itself is not surfaced as a user-managed link.
2. **No Google email recorded as an integration link.** Google login mints the session, but there is no row that says "this Forage user has linked Google account `kasper@understory.io`" that can be unlinked or relinked separately from the primary auth identity.
3. **Asymmetric UX.** Slack is the only provider with a "Linked accounts" card on `/settings/account`. GitHub and Google get no such card, so users cannot see, manage, add additional, or disconnect those identities.
4. **No reuse downstream.** Future features — mapping commits → users, sending Google-Calendar release windows, etc. — need a single authoritative record of "user X has linked external account Y on provider Z". Today, only Slack has that record.

## Separation of Concerns

**Forest** (upstream gRPC server) — **single source of truth for OAuth identities**:
- Owns the `identities` table (`provider`, `provider_user_id`, `provider_email`, `provider_data`) covering GitHub, Google, GitLab, Microsoft, magic-link.
- Exposes `LinkOAuthProvider` / `UnlinkOAuthProvider` RPCs (already implemented).
- Records the identity used at login automatically (existing behaviour — to be verified during Phase 2).
- Slack is not part of Forest's `OAuthProvider` enum and remains entirely in Forage.

**Forage** (this codebase — the BFF):
- **Does not store GitHub/Google linkage itself.** Reads via Forest's gRPC client; writes via `LinkOAuthProvider` / `UnlinkOAuthProvider`.
- Drives the OAuth dance for *linking* (separate from *login*), with `purpose=link` state so the callback calls `LinkOAuthProvider` on Forest instead of minting a session.
- Owns the **Slack** user-link table (`slack_user_links`) as today — Forest has no concept of Slack identities.
- Renders the unified "Linked accounts" UI on the account page by merging Forest's identities (github/google) with Forage's slack_user_links into a single view model.

This eliminates the duplicate-source-of-truth risk: GitHub/Google live exactly once, in Forest. Forage is a thin proxy for them.

## Scope

In scope:
- **Forage → Forest gRPC client glue**: thin Forage-side wrapper around `LinkOAuthProvider`, `UnlinkOAuthProvider`, and identity listing (likely already exposed via the user RPC — verify during Phase 2 and add a `ListLinkedIdentities` RPC if missing).
- **GitHub link flow**: `/settings/account/github/{connect,callback,disconnect}` with `read:user user:email` scopes. Exchanges code for token, fetches profile, calls Forest's `LinkOAuthProvider` with provider=`GITHUB`, `provider_user_id`, `provider_email`, and `provider_data` JSON (`{login, name, avatar_url}`).
- **Google link flow**: `/settings/account/google/{connect,callback,disconnect}` with `openid email profile` scopes. Same pattern; provider=`GOOGLE`, `provider_data` carries `{sub, name, picture, email_verified}`.
- **Account-page UI**: New "Linked accounts" section that renders cards for GitHub, Google, and the existing Slack workspaces. Conditional on each provider's OAuth being configured (`has_github_oauth`, `has_google_oauth`, `has_slack_oauth`).
- **Auto-link on login**: verify that Forest already calls `LinkOAuthProvider` (or its internal equivalent) on first OAuth sign-in. If not, add the call into the existing Forage `/auth/{provider}/callback` login path.
- **CSRF + OAuth state**: `state` carries `purpose=link|login` plus the `user_id` (when linking) and a CSRF nonce. Callbacks reject mismatches with 403.
- **Disconnect**: POST with CSRF calls Forest's `UnlinkOAuthProvider`. Confirmation copy warns "You'll need this account to sign back in if it's your only login method." No client-side block on last-method-disconnect (deferred until Forest exposes auth-method enumeration).
- **Both providers ship together** as a single slice — they share the state codec, the gRPC wrapper, the UI partial, and the route shape.

Out of scope:
- Adding more providers (GitLab, Microsoft, Discord) — Forest's enum supports them; this spec ships two.
- Migrating `slack_user_links` into Forest — Slack is not in Forest's OAuth provider enum, and bringing it in is a separate, larger spec.
- Surfacing linked identities to other Forage features (commit→user mapping, calendar integrations) — future specs.
- Token refresh / long-lived access (identity-only).
- Editing display name post-creation — disconnect + reconnect to refresh.
- Multi-link per provider (GitHub/Google) — one per Forage user; revisit when a concrete personal+work use case lands.
- Step-up auth on disconnect — current threat model treats CSRF + active session as sufficient.

## Architecture

### Storage (Forest, via gRPC)

GitHub/Google linkage lives in Forest's existing `identities` table — **no new Forage table**. The relevant Forest proto surface (`apps/forest/interface/proto/forest/v1/users.proto`):

```proto
enum OAuthProvider {
  OAUTH_PROVIDER_UNSPECIFIED = 0;
  OAUTH_PROVIDER_GITHUB      = 1;
  OAUTH_PROVIDER_GOOGLE      = 2;
  OAUTH_PROVIDER_GITLAB      = 3;
  OAUTH_PROVIDER_MICROSOFT   = 4;
  OAUTH_PROVIDER_MAGIC_LINK  = 5;
}

rpc LinkOAuthProvider(LinkOAuthProviderRequest)     returns (LinkOAuthProviderResponse);
rpc UnlinkOAuthProvider(UnlinkOAuthProviderRequest) returns (UnlinkOAuthProviderResponse);
```

**Verification tasks for Phase 2 (must happen before tests):**
1. Confirm there's a list RPC (e.g. `ListLinkedIdentities` / `GetUser` returning identities). If absent, add one — server-side scope, included in this spec's slice.
2. Confirm the OAuth login path (`/auth/github`, `/auth/google` in Forage) already records the identity in Forest — either by Forest doing it during signup, or by Forage explicitly calling `LinkOAuthProvider`. If neither, wire the call in `forage-server`'s login callback.
3. Confirm `UnlinkOAuthProvider` rejects unlinking the only remaining auth method (or doesn't — either is workable, but the behaviour shapes the disconnect UX copy).

### Domain Model (`forage-core`)

```rust
/// Forage's view of a linked external identity (provider-agnostic).
#[derive(Debug, Clone)]
pub struct LinkedIdentity {
    pub provider: LinkedProvider,
    pub external_id: String,        // github numeric id, google sub, slack user_id
    pub display_name: String,       // login / username (e.g. "kjuulh")
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkedProvider {
    GitHub,
    Google,
    Slack, // sourced from Forage's slack_user_links, not Forest
}
```

`LinkedIdentity` is the **render model** for the account page. It's assembled in the route handler by merging:
- Forest's identities (GitHub, Google) via gRPC
- Forage's `slack_user_links` rows (existing path)

The pure mappers `github_profile_to_link_request(profile) -> LinkOAuthProviderRequest` and `google_userinfo_to_link_request(userinfo) -> LinkOAuthProviderRequest` live in `forage-core` and are unit-tested without I/O.

### Forest Client Wrapper (`forage-server`)

```rust
/// Thin Forage-side facade over Forest's identity RPCs.
#[async_trait]
pub trait LinkedIdentityClient: Send + Sync {
    async fn list(&self, user_id: &str) -> Result<Vec<LinkedIdentity>, ClientError>;
    async fn link(&self, user_id: &str, req: LinkOAuthProviderRequest) -> Result<(), ClientError>;
    async fn unlink(&self, user_id: &str, provider: LinkedProvider, external_id: &str)
        -> Result<(), ClientError>;
}
```

- `ForestLinkedIdentityClient` is the real impl (talks to Forest via tonic).
- `MemoryLinkedIdentityClient` is the test double used in `forage-server` route tests.
- No Forage DB migration required.

### Routes (`forage-server`)

| Route | Method | Auth | Description |
|-------|--------|------|-------------|
| `GET  /settings/account/github/connect` | GET | Session | Redirect to GitHub authorize URL with `state=purpose:link,...` |
| `GET  /settings/account/github/callback` | GET | Session | Exchange code, fetch `/user` + `/user/emails`, upsert link |
| `POST /settings/account/github/disconnect` | POST | Session + CSRF | Delete link by `external_id` |
| `GET  /settings/account/google/connect` | GET | Session | Redirect to Google consent screen with linking state |
| `GET  /settings/account/google/callback` | GET | Session | Exchange code, fetch userinfo, upsert link |
| `POST /settings/account/google/disconnect` | POST | Session + CSRF | Delete link by `external_id` |

Existing `/auth/github` and `/auth/google` (login flows) are **modified** to:
1. After successful session mint, look up the user in `account_links` by `(provider, external_id)`.
2. If no row exists, upsert one with the OAuth profile data.
3. If a row exists but belongs to a **different** user_id, log a warning and skip (cross-user takeover prevention; covered by edge cases below).

### OAuth `state` Encoding

We need a single callback per provider that handles both login and linking. Encode state as a signed JSON blob (existing `forage-core` session signing util) carrying:

```rust
struct OAuthState {
    nonce: String,
    purpose: OAuthPurpose, // Login | LinkAccount { user_id }
    return_to: Option<String>,
}
```

- The callback decodes + verifies the signature, then dispatches: login → existing path; link → new path that requires the session's `user_id == state.user_id`.
- Signature key reused from the existing OAuth state secret.

### Templates

| Template | Status | Description |
|----------|--------|-------------|
| `pages/account.html.jinja` | Update | Single "Linked accounts" section rendering all providers via a `linked_accounts` list. Slack workspace cards continue to render their workspace + username; GitHub/Google render `@login` (+ email subtext). Each card has a Disconnect button. Below the cards: "Add" buttons for each configured provider with no link yet (or always-shown for multi-link providers). |
| `pages/account.html.jinja` | Update | "Notification preferences" table gains no new columns (channel matrix is org-level; spec 006 owns it). |
| `partials/account_link_card.html.jinja` | New | Reusable card component: icon + display_name + email (optional) + disconnect form. |

### Route Handler Outline (GitHub linking, illustrative)

```rust
// GET /settings/account/github/connect
async fn github_link_start(state, session) -> Result<Redirect> {
    let cfg = state.github_oauth_config.as_ref().ok_or(not_configured)?;
    let oauth_state = sign(OAuthState {
        nonce: random_nonce(),
        purpose: OAuthPurpose::LinkAccount { user_id: session.user.user_id.clone() },
        return_to: Some("/settings/account".into()),
    });
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}/auth/github/callback&scope=read:user%20user:email&state={}",
        cfg.client_id, cfg.redirect_host, urlencoding::encode(&oauth_state),
    );
    Ok(Redirect::to(&url))
}

// GET /auth/github/callback (existing — modified)
async fn github_oauth_callback(state, session_opt, Query(params)) -> Response {
    let oauth_state = verify(&params.state)?;        // 403 on bad sig / expired
    let token = exchange_code(&params.code).await?;  // 502 on Github API failure
    let profile = fetch_github_profile(&token).await?;

    match oauth_state.purpose {
        OAuthPurpose::Login => login_or_signup(profile, state).await,
        OAuthPurpose::LinkAccount { user_id } => {
            let current = session_opt.ok_or(unauthorized)?;
            if current.user.user_id != user_id {
                return forbidden("link target mismatch");
            }
            let req = github_profile_to_link_request(&profile); // pure
            match state.linked_identity_client.link(&user_id, req).await {
                Ok(()) => Redirect::to(oauth_state.return_to.as_deref().unwrap_or("/settings/account")),
                Err(ClientError::AlreadyLinkedToAnotherUser) => conflict(
                    "this GitHub account is already linked to another Forage user"),
                Err(ClientError::AlreadyLinkedSameUser) => Redirect::to("/settings/account?flash=already_linked"),
                Err(e) => internal_error(&state, "github link", &e),
            }
        }
    }
}
```

### Account Page Layout

```
Linked accounts
Link external identities so we can map your work to your account.

┌─────────────────────────────────────────────────┐
│ 🐙 GitHub · @kjuulh                              │
│ kasper@understory.io                 [Disconnect]│
└─────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────┐
│ 🟦 Google · Kasper Hermansen                     │
│ kasper@understory.io                 [Disconnect]│
└─────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────┐
│ 🟣 Slack · rawpotion · @kjuulh                   │
│                                       [Disconnect]│
└─────────────────────────────────────────────────┘

[+ Add GitHub]  [+ Add Google]  [+ Add Slack workspace]
```

The "Add" button for a provider is shown when:
- GitHub / Google: no link of that provider exists yet, OR the provider supports multi-link (set to false by default — single link per provider).
- Slack: always shown (multi-workspace supported by existing code).

### Environment / Config

Reuses existing config:
- `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `GITHUB_REDIRECT_HOST`
- `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `GOOGLE_REDIRECT_HOST`
- `INTEGRATION_ENCRYPTION_KEY`
- `OAUTH_STATE_SIGNING_KEY` (existing; if not currently a separate key, falls back to `SESSION_SIGNING_KEY`)

No new secrets required — the existing OAuth apps already cover login and can serve linking with the same callback URL.

## Behavioral Contract

### Linking flow (per provider)
- Unauthenticated user hitting `/settings/account/{provider}/connect` → 302 to `/login` (existing middleware).
- Authenticated user → 302 to provider authorize URL with signed state encoding `purpose=link` + their `user_id`.
- Provider redirects to existing `/auth/{provider}/callback` with `state` + `code`.
- Bad / expired / forged state → 403 with "OAuth state invalid" page; no DB writes.
- `state.user_id != session.user_id` → 403; no DB writes.
- Provider returns an `error` query param → render error page with "Try again" link to `/settings/account`.
- Token exchange or profile fetch network failure → 502; structured log; no DB writes.
- `external_id` already linked to a **different** Forage user → 409 conflict page; no DB writes; clear copy explaining the situation.
- `external_id` already linked to the **same** Forage user → idempotent upsert (updates `display_name`, `email`, `avatar_url`, `updated_at`).
- Successful link → upsert row, redirect to `/settings/account` with flash banner "GitHub account linked".

### Auto-link on OAuth login
- After successful session mint, if `(provider, external_id)` has no `account_links` row, upsert one.
- If a row exists for a **different** user (e.g. account merged elsewhere), log `WARN` and skip — do not overwrite.
- Auto-link failures do not block login (best-effort write).

### Disconnect flow
- POST without valid CSRF → 403; no DB writes.
- POST with valid CSRF + matching `external_id` → delete row; redirect with flash "GitHub account unlinked".
- POST referencing an `external_id` not owned by the session user → 404; no DB writes (prevents enumeration).
- Disconnecting **does not** terminate the current session even if that provider was used to sign in. Copy on the disconnect button clarifies: "You'll need this account to sign back in if it's your only login method."

### Multi-link policy
- GitHub: **one** link per Forage user. Attempting to add a second returns 409 with "You already have a GitHub account linked. Disconnect it first to switch."
- Google: **one** link per Forage user. Same behaviour.
- Slack: **many** per user (existing behaviour preserved).
- Rationale: GitHub/Google identities are typically singular per person; multi-link adds UX surface area and conflict-resolution complexity we don't need yet. Revisit when a concrete use case emerges.

### Account page rendering
- Section hidden entirely if **no** OAuth providers are configured AND user has no existing links.
- Per-provider "Add" button hidden if the provider's OAuth config is missing (`state.{provider}_oauth_config.is_none()`).
- Each card shows: provider icon, display name, email (if available), disconnect button.
- Disconnect button is a `POST` form with hidden `_csrf` + `external_id`.
- All copy uses sentence case, matching existing account-page conventions.

### Security
- OAuth state is signed (HMAC) and includes a 10-minute expiry timestamp.
- `redirect_uri` registered with each provider matches exactly; we do not parameterise it from the request.
- CSRF token validated on disconnect endpoints (form POST).
- `email` from Google is accepted only when `email_verified=true` in the userinfo response; otherwise stored as `None`.
- `email` from GitHub uses the primary verified email from `/user/emails`; otherwise stored as `None`.
- We **do not** persist OAuth access tokens for GitHub/Google in this spec (identity-only). If a future feature requires API calls, it adds token storage in a follow-up spec with explicit scopes and encryption review.

### Non-functional
- DB writes per OAuth callback: ≤ 2 (session row + account_link row).
- Provider API calls per link: GitHub = 2 (`/user`, `/user/emails`); Google = 1 (`userinfo`).
- All HTTP calls to providers have a 10s timeout and are retried 0 times (user-initiated; failure is acceptable).

## Edge Case Catalogue

| # | Scenario | Expected |
|---|----------|----------|
| 1 | Linking provider when OAuth config missing | 404 "Provider not configured" |
| 2 | Unauthenticated `/settings/account/github/connect` | 302 to `/login?next=...` |
| 3 | `state` signature invalid | 403 OAuth error page |
| 4 | `state` expired (>10 min) | 403 OAuth error page |
| 5 | `state.user_id` ≠ session `user_id` | 403 with "Link target mismatch" |
| 6 | Provider returns `error=access_denied` | Redirect to `/settings/account?error=access_denied` |
| 7 | Token exchange returns 5xx | 502 with retry CTA |
| 8 | `external_id` linked to another Forage user | 409 conflict page |
| 9 | `external_id` already linked to same user (re-link) | Idempotent update, success flash |
| 10 | User already has a GitHub link, tries to add another | 409 "Disconnect existing first" |
| 11 | Slack: user adds a second workspace | Success (multi-link preserved) |
| 12 | Google `email_verified=false` | Link created without email |
| 13 | GitHub user has no public email AND `/user/emails` returns none verified | Link created without email |
| 14 | Disconnect with wrong CSRF | 403 |
| 15 | Disconnect with `external_id` not owned by user | 404 (no enumeration) |
| 16 | Disconnect succeeds; session still valid | User remains signed in |
| 17 | Auto-link on login: row already exists for current user | Idempotent update |
| 18 | Auto-link on login: row exists for **different** user | Warn-log, skip, login still succeeds |
| 19 | Provider API timeout | 502 with retry CTA |
| 20 | Concurrent connect from same user (double-click) | Second hits unique constraint; downgraded to idempotent update or surfaced as 409 if mid-flight |
| 21 | XSS via `display_name` (e.g. GitHub login `<script>`) | Rendered through MiniJinja autoescape; no raw HTML output |
| 22 | Avatar URL not HTTPS | Stored but rendered with `loading="lazy"` and through a sanitiser; non-https avatars fall back to initials |
| 23 | Slack section hidden when no `slack_config` present | Existing behaviour preserved |
| 24 | Account page with mixture of configured + unconfigured providers | Renders cards for what exists; "Add" buttons only for configured |

## Verification Architecture

### Purity boundary
- **Pure core (`forage-core`)**:
  - `AccountProvider`, `AccountLink` types
  - `OAuthState::{sign, verify}` (HMAC + expiry check)
  - `github_profile_to_link(user_id, GithubProfile) -> AccountLink`
  - `google_profile_to_link(user_id, GoogleUserinfo) -> AccountLink`
  - `email_from_github(profile, emails) -> Option<String>` (picks primary + verified)
  - `email_from_google(userinfo) -> Option<String>` (gated on `email_verified`)
- **Effectful shell (`forage-server`)**:
  - HTTP calls to GitHub / Google APIs
  - Session lookup, CSRF check
  - DB writes via `AccountLinkStore`
  - Redirect / template rendering
- **DB layer (`forage-db`)**:
  - `PgAccountLinkStore` with sqlx queries
  - Encryption of `metadata_encrypted` via existing util

### Provable properties (property-based tests)
- `verify(sign(s)) == s` for any well-formed `OAuthState`
- `verify(tampered(sign(s)))` always errors
- `verify(sign(s))` after expiry always errors
- `email_from_github` returns `None` when no emails are both `primary` and `verified`
- `email_from_google` returns `None` when `email_verified=false`

### Testing strategy
- **Unit tests in `forage-core`**: pure mappers + state codec
- **Integration tests in `forage-server`**: route tests using `MemoryAccountLinkStore` + mocked OAuth HTTP via `wiremock`
- **sqlx integration**: `PgAccountLinkStore` round-trip tests in `forage-db` against the test DB
- **Manual**: end-to-end GitHub + Google flow against real OAuth apps in dev environment

## Implementation Order

GitHub and Google ship together as a single slice — they share the state codec, the gRPC client, the UI partial, and the route shape. Splitting would duplicate work.

### Phase A: Forest-side verification (Phase 2 prerequisite)
1. Audit `users.proto` + `grpc/users.rs` for an existing `ListLinkedIdentities` (or equivalent on `GetUser`). If missing, add the RPC + server impl.
2. Audit the existing OAuth login path. Confirm it calls `LinkOAuthProvider` on first sign-in. If not, file the gap and decide whether Forest adds it or Forage explicitly calls the RPC after session mint.
3. Confirm `LinkOAuthProvider` semantics on duplicate external_id (idempotent vs error) and on linking an external_id owned by another user (rejects vs takes over). Document the actual behaviour.

### Phase B: Pure core (`forage-core`)
1. `LinkedProvider`, `LinkedIdentity` types
2. `OAuthState` + `sign`/`verify` (extend existing if already present in `forage-core`)
3. `github_profile_to_link_request`, `google_userinfo_to_link_request` pure mappers
4. `email_from_github`, `email_from_google` (verified-email pickers)
5. Tests: types, codec round-trip, mappers, email pickers

### Phase C: Forest client wrapper (`forage-server`)
1. `LinkedIdentityClient` trait + `ForestLinkedIdentityClient` real impl
2. `MemoryLinkedIdentityClient` test double
3. Map Forest's gRPC errors → `ClientError` variants (`AlreadyLinkedToAnotherUser`, `AlreadyLinkedSameUser`, `NotFound`, `Transport`)
4. Tests: error mapping per Forest response code

### Phase D: Linking routes — both providers in one slice
1. `/settings/account/{github,google}/connect` — redirect with signed state, link purpose
2. Extend `/auth/{github,google}/callback` to dispatch on `state.purpose`
3. `/settings/account/{github,google}/disconnect` (POST + CSRF) calling `UnlinkOAuthProvider`
4. Google: `email_verified=true` gating in the mapper
5. Tests: full edge-case catalogue 1–24 for both providers

### Phase E: Account page UI
1. Update `account.html.jinja` with unified "Linked accounts" section
2. Add `partials/account_link_card.html.jinja`
3. Route handler merges `linked_identity_client.list(user_id)` (github + google) with existing `slack_user_links` into one `linked_accounts` view-model list
4. "Add" buttons hidden per provider when (a) provider OAuth not configured OR (b) for GitHub/Google, a link already exists
5. Flash banners for success / conflict / error
6. Tests: template renders for all combinations of configured providers × existing links

### Phase F: Auto-link verification
1. Confirm via integration test that signing in with GitHub creates the identity row (either Forest does it natively or Forage's login callback calls `LinkOAuthProvider`).
2. If the gap from Phase A.2 needs Forage to wire the call, add it here.
3. Behaviour on conflicting external_id at login: warn-log, do not break login.

### Phase G: Hardening
1. clippy clean across `forage-core`, `forage-server`
2. cargo-audit clean
3. Property tests on state codec + email pickers
4. Adversarial review documented in `specs/reviews/010-adversarial-review.md`

## Test Catalogue (~30 tests)

**Pure core (`forage-core`) — 8**
- `OAuthState` round-trips through sign+verify
- Tampered state fails verification
- Expired state fails verification
- `github_profile_to_link` populates all fields
- `email_from_github` picks primary verified
- `email_from_github` returns None when no verified
- `google_profile_to_link` honours `email_verified`
- `AccountProvider::as_str` round-trips from string

**Forest client wrapper (`forage-server`) — 5**
- `list` returns identities for a user (mocked Forest response)
- `link` succeeds → propagates Ok
- `link` returns "already linked to another user" → `ClientError::AlreadyLinkedToAnotherUser`
- `link` returns "already linked to same user" → `ClientError::AlreadyLinkedSameUser`
- `unlink` for non-existent identity → `ClientError::NotFound`

**GitHub routes (`forage-server`) — 7**
- Unauthenticated connect → 302 to login
- Authenticated connect → 302 to github.com with signed state
- Callback with valid state + new user → 200, link created
- Callback with mismatched user_id → 403
- Callback with provider error → error page
- Disconnect with CSRF → row deleted
- Second link attempt → 409

**Google routes (`forage-server`) — 5**
- Mirror of github happy path
- `email_verified=false` → link without email
- Disconnect happy path
- Disconnect wrong CSRF → 403
- Conflict on duplicate

**Account page (`forage-server`) — 3**
- Renders all configured providers + existing links
- Hides cards/buttons for unconfigured providers
- Mixed state (one link present, one missing) renders correctly

**Auto-link — 2**
- Login auto-creates link when missing
- Login skips and warns when external_id belongs to another user

## Resolved Decisions

These were settled during Phase 1 spec authoring; recorded here so future readers don't re-litigate them:

1. **Storage**: Forest's `identities` table via `LinkOAuthProvider`/`UnlinkOAuthProvider` RPCs. No new Forage table. Slack remains in Forage (`slack_user_links`) because it's not in Forest's `OAuthProvider` enum.
2. **Auto-link on login**: yes — signing in with a provider records the identity. User already consented by signing in; making them click again is friction with no security benefit.
3. **Multi-link policy**: one GitHub + one Google per Forage user. Slack stays multi-workspace. Revisit when a concrete personal+work use case lands.
4. **Disconnect**: allow with warning copy. No step-up auth, no client-side "last method" block. Matches existing Slack-disconnect UX; CSRF + active session is sufficient in the current threat model.
5. **Slicing**: GitHub and Google ship together as one slice.

## Open Questions for Phase 2 Review

1. **Does Forest already auto-link on first OAuth sign-in?** Phase A.2 must answer this before tests are written. The answer determines whether Phase F is a verification step or includes new code in `forage-server`.
2. **Does Forest expose a list-linked-identities RPC?** Phase A.1 must answer this. If absent, the RPC is added under this spec's scope (server-side).
3. **What does `LinkOAuthProvider` do when the external_id is already owned by another user?** Need exact behaviour to map to `ClientError` variants in Phase C.
4. **Should the UI show "primary login method" markers per linked account?** Defer to a follow-up — useful but not required for parity with Slack today.
5. **Should we surface `last_used_at` per link?** Defer.
