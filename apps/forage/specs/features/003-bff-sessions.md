# Spec 003: BFF Session Management

## Status: Phase 2 Complete (34 tests passing)

## Problem

The current auth implementation stores forest-server's raw access_token and
refresh_token directly in browser cookies. This has several problems:

1. **Security**: Forest-server credentials are exposed to the browser. If XSS
   ever bypasses HttpOnly (or we need to read auth state client-side), the raw
   tokens are right there.
2. **No transparent refresh**: The extractor checks cookie existence but can't
   detect token expiry. When the access_token expires, `get_user()` fails and
   the user gets redirected to login - even though the refresh_token is still
   valid. Users get randomly logged out.
3. **No user caching**: Every authenticated page makes 2-3 gRPC round-trips
   (token_info + get_user + page-specific call). For server-rendered pages
   this is noticeable latency.
4. **No session concept**: There's no way to list active sessions, revoke
   sessions, or track "last seen". The server is stateless in a way that
   hurts the product.

## Solution: Backend-for-Frontend (BFF) Sessions

Forage server owns sessions. The browser gets an opaque session ID cookie.
Forest-server tokens and cached user info live server-side only.

```
Browser  --[forage_session cookie]--> forage-server --[access_token]--> forest-server
                                          |
                                    [session store]
                                    sid -> { access_token, refresh_token,
                                             expires_at, user_cache }
```

## Architecture

### Session Store Trait

A trait in `forage-core` so the store is swappable and testable:

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, data: SessionData) -> Result<SessionId, SessionError>;
    async fn get(&self, id: &SessionId) -> Result<Option<SessionData>, SessionError>;
    async fn update(&self, id: &SessionId, data: SessionData) -> Result<(), SessionError>;
    async fn delete(&self, id: &SessionId) -> Result<(), SessionError>;
}
```

### SessionId

An opaque, cryptographically random token. Not a UUID - use 32 bytes of
`rand::OsRng` encoded as base64url. This is the only thing the browser sees.

### SessionData

```rust
pub struct SessionData {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: chrono::DateTime<Utc>,  // computed from expires_in_seconds
    pub user: Option<CachedUser>,                   // cached to avoid repeated get_user calls
    pub created_at: chrono::DateTime<Utc>,
    pub last_seen_at: chrono::DateTime<Utc>,
}

pub struct CachedUser {
    pub user_id: String,
    pub username: String,
    pub emails: Vec<UserEmail>,
}
```

### In-Memory Store (Phase 1)

`HashMap<SessionId, SessionData>` behind a `RwLock`. Good enough for single-instance
deployment. A background task reaps expired sessions periodically.

This is sufficient for now. When forage needs horizontal scaling, swap to a
Redis or PostgreSQL-backed store behind the same trait.

### Cookie

Single cookie: `forage_session`
- Value: the opaque SessionId (base64url, ~43 chars)
- HttpOnly: yes
- Secure: yes (always - even if we need to configure for local dev)
- SameSite: Lax
- Path: /
- Max-Age: 30 days (the session lifetime, not the access token lifetime)

The previous `forage_access` and `forage_refresh` cookies are removed entirely.

## Behavioral Contract

### Login / Register Flow

1. User submits login/signup form
2. Forage calls forest-server's Login/Register RPC, gets AuthTokens
3. Forage computes `access_expires_at = now + expires_in_seconds`
4. Forage calls `get_user` to populate the user cache
5. Forage creates a session in the store with tokens + user cache
6. Forage sets `forage_session` cookie with the session ID
7. Redirect to /dashboard

### Authenticated Request Flow

1. Extract `forage_session` cookie
2. Look up session in store
3. If no session: redirect to /login
4. If `access_expires_at` is in the future (with margin): use cached access_token
5. If access_token is expired or near-expiry (< 60s remaining):
   a. Call forest-server's RefreshToken RPC with the stored refresh_token
   b. On success: update session with new tokens + new expiry
   c. On failure (refresh_token also expired): delete session, redirect to /login
6. Return session to the route handler (which has access_token + cached user)

### Logout Flow

1. Extract session ID from cookie
2. Get refresh_token from session store
3. Call forest-server's Logout RPC (best-effort)
4. Delete session from store
5. Clear the `forage_session` cookie
6. Redirect to /

### Session Expiry

- Sessions expire after 30 days of inactivity (configurable)
- `last_seen_at` is updated on each request
- A background reaper runs every 5 minutes, removes sessions where
  `last_seen_at + 30 days < now`
- If the refresh_token is rejected by forest-server, the session is
  destroyed immediately regardless of age

## Changes to Existing Code

### What Gets Replaced

- `auth.rs`: `MaybeAuth` and `RequireAuth` extractors rewritten to use session store
- `auth.rs`: `auth_cookies()` and `clear_cookies()` replaced with session cookie helpers
- `routes/auth.rs`: Login/signup handlers create sessions instead of setting token cookies
- `routes/auth.rs`: Logout handler destroys session
- `routes/auth.rs`: Dashboard and token pages use `session.user` cache instead of calling `get_user()` every time

### What Stays the Same

- `ForestAuth` trait and `GrpcForestClient` - unchanged, still the interface to forest-server
- Validation functions in `forage-core` - unchanged
- Templates - unchanged (they receive the same data)
- Route structure and URLs - unchanged
- All existing tests continue to pass (mock gets wrapped in mock session store)

### New Dependencies

- `rand` (workspace): for cryptographic session ID generation
- No new external session framework - the store is simple enough to own

### AppState Changes

```rust
pub struct AppState {
    pub templates: TemplateEngine,
    pub forest_client: Arc<dyn ForestAuth>,
    pub sessions: Arc<dyn SessionStore>,  // NEW
}
```

## Extractors (New Design)

### `Session` extractor (replaces `RequireAuth`)

Extracts the session, handles refresh transparently, provides both the
access_token (for forest-server calls that aren't cached) and cached user info.

```rust
pub struct Session {
    pub session_id: SessionId,
    pub access_token: String,
    pub user: CachedUser,
}
```

The extractor:
1. Reads cookie
2. Looks up session
3. Refreshes token if needed (updating the store)
4. Returns `Session` or redirects to /login

Because refresh updates the session store (not the cookie), no response
headers need to be set during extraction. The cookie stays the same.

### `MaybeSession` extractor (replaces `MaybeAuth`)

Same as `Session` but returns `Option<Session>` instead of redirecting.
Used for pages like /signup and /login that behave differently when
already authenticated.

## Edge Cases

- **Concurrent requests during refresh**: Two requests arrive with the same
  expired access_token. Both try to refresh. The session store update is
  behind a RwLock, so the second one will see the already-refreshed token.
  Alternatively, use a per-session Mutex for refresh operations to avoid
  double-refresh. Start simple (accept occasional double-refresh), optimize
  if it becomes a problem.

- **Session ID collision**: 32 bytes of crypto-random = 256 bits of entropy.
  Collision probability is negligible.

- **Store grows unbounded**: The reaper task handles this. For in-memory store,
  also enforce a max session count (e.g., 100k) as a safety valve.

- **Server restart loses all sessions**: Yes. In-memory store is not durable.
  All users will need to re-login after a deploy. This is acceptable for now
  and is the primary motivation for eventually moving to Redis/Postgres.

- **Cookie without valid session**: Treat as unauthenticated. Clear the stale
  cookie.

- **Forest-server down during refresh**: Keep the existing session alive with
  the expired access_token. The next forest-server call will fail, and the
  route handler deals with it (same as today). Don't destroy the session just
  because refresh failed due to network - only destroy it if forest-server
  explicitly rejects the refresh token.

## Test Strategy

### Unit Tests (forage-core)

- `SessionId` generation: length, format, uniqueness (generate 1000, assert no dupes)
- `SessionData` expiry logic: `is_access_expired()`, `needs_refresh()` (with margin)
- `InMemorySessionStore`: create/get/update/delete round-trip
- `InMemorySessionStore`: get non-existent returns None
- `InMemorySessionStore`: delete then get returns None

### Integration Tests (forage-server)

All existing tests must continue passing. Additionally:

- Login creates a session and sets `forage_session` cookie (not `forage_access`)
- Dashboard with valid session cookie returns 200 with user content
- Dashboard with expired access_token (but valid refresh) still returns 200
  (transparent refresh)
- Dashboard with expired session redirects to /login
- Logout destroys session and clears cookie
- Signup creates session same as login
- Old `forage_access` / `forage_refresh` cookies are ignored (clean break)

### Mock Session Store

For tests, use `InMemorySessionStore` directly (it's already simple). The mock
`ForestClient` stays as-is for controlling gRPC behavior.

## Implementation Order

1. Add `SessionId`, `SessionData`, `SessionStore` trait, `InMemorySessionStore` to `forage-core`
2. Add unit tests for session types and in-memory store
3. Add `rand` dependency, implement `SessionId::generate()`
4. Rewrite `auth.rs` extractors to use session store
5. Rewrite route handlers to use new extractors
6. Update `AppState` to include session store
7. Update `main.rs` to create the in-memory store
8. Update integration tests
9. Add session reaper background task
10. Remove old cookie helpers and constants

## What This Does NOT Do

- No Redis/Postgres session store yet (in-memory only)
- No "active sessions" UI for users
- No CSRF protection (SameSite=Lax is sufficient for form POSTs from same origin)
- No session fixation protection beyond generating new IDs on login
- No rate limiting on session creation (defer to forest-server's rate limiting)

## Open Questions

1. Should we invalidate all sessions for a user when they change their password?
   (Requires either forest-server notification or polling.)
2. Session cookie name: `forage_session` or `__Host-forage_session`?
   (`__Host-` prefix forces Secure + no Domain + Path=/, which is stricter.)
3. Should the user cache have a separate TTL (e.g., refresh user info every 5 min)?
   Or only refresh on explicit actions like "edit profile"?
