# Spec 002: Authentication (Forest-Server Frontend)

## Status: Phase 2 Complete (20 tests passing)

## Overview

Forage is a server-side rendered frontend for forest-server. All user management
(register, login, sessions, tokens) is handled by forest-server's UsersService
via gRPC. Forage stores access/refresh tokens in HTTP-only cookies and proxies
auth operations to the forest-server backend.

## Architecture

```
Browser <--HTML/cookies--> forage-server (axum) <--gRPC--> forest-server (UsersService)
```

- No local user database in forage
- forest-server owns all auth state (users, sessions, passwords)
- forage-server stores access_token + refresh_token in HTTP-only cookies
- forage-server has a gRPC client to forest-server's UsersService

## Behavioral Contract

### gRPC Client (`forage-core`)

A typed client wrapping forest-server's UsersService:
- `register(username, email, password) -> Result<AuthTokens>`
- `login(identifier, password) -> Result<AuthTokens>`
- `refresh_token(refresh_token) -> Result<AuthTokens>`
- `logout(refresh_token) -> Result<()>`
- `get_user(access_token) -> Result<User>`
- `list_personal_access_tokens(access_token, user_id) -> Result<Vec<Token>>`
- `create_personal_access_token(access_token, user_id, name, scopes, expires) -> Result<(Token, raw_key)>`
- `delete_personal_access_token(access_token, token_id) -> Result<()>`

### Cookie Management

- `forage_access` cookie: access_token, HttpOnly, Secure, SameSite=Lax, Path=/
- `forage_refresh` cookie: refresh_token, HttpOnly, Secure, SameSite=Lax, Path=/
- On every authenticated request: extract access_token from cookie
- If access_token expired but refresh_token valid: auto-refresh, set new cookies
- If both expired: redirect to /login

### Routes

#### Public Pages
- `GET /signup` -> signup form (200), redirect to /dashboard if authenticated
- `POST /signup` -> call Register RPC, set cookies, redirect to /dashboard (302)
- `GET /login` -> login form (200), redirect to /dashboard if authenticated
- `POST /login` -> call Login RPC, set cookies, redirect to /dashboard (302)
- `POST /logout` -> call Logout RPC, clear cookies, redirect to / (302)

#### Authenticated Pages
- `GET /dashboard` -> home page showing user info + orgs (200), or redirect to /login
- `GET /settings/tokens` -> list PATs (200)
- `POST /settings/tokens` -> create PAT, show raw key once (200)
- `POST /settings/tokens/:id/delete` -> delete PAT, redirect to /settings/tokens (302)

### Error Handling
- gRPC errors mapped to user-friendly messages in form re-renders
- Invalid credentials: "Invalid username/email or password" (no enumeration)
- Duplicate email/username on register: "Already registered"
- Network error to forest-server: 502 Bad Gateway page

## Edge Cases
- Forest-server unreachable: show error page, don't crash
- Expired access token with valid refresh: auto-refresh transparently
- Both tokens expired: redirect to login, clear cookies
- Malformed cookie values: treat as unauthenticated
- Concurrent requests during token refresh: only refresh once

## Purity Boundary

### Pure Core (`forage-core`)
- ForestClient trait (mockable for tests)
- Token cookie helpers (build Set-Cookie headers, parse cookies)
- Form validation (email format, password length)

### Effectful Shell (`forage-server`)
- Actual gRPC calls to forest-server
- HTTP cookie read/write
- Route handlers and template rendering
- Auth middleware (extractor)

## Test Strategy

### Unit Tests (forage-core)
- Cookie header building: correct flags, encoding
- Form validation: email format, password length
- Token expiry detection

### Integration Tests (forage-server)
- All routes render correct templates (using mock ForestClient)
- POST /signup calls register, sets cookies on success
- POST /login calls login, sets cookies on success
- GET /dashboard without cookies -> redirect to /login
- GET /dashboard with valid token -> 200 with user content
- POST /logout clears cookies
- Error paths: bad credentials, server down

The mock ForestClient allows testing all UI flows without a running forest-server.
