# 022: Forest Device Login — `gh auth login`-style web flow

Status: In-flight. Server-side, DB, CLI (beads 022.1–022.7) and FOREST_PROFILE
`web=` parser shipped; forage `/device` route and full end-to-end docs still
pending. See §8 for slice-level status.

## 0. Why

Today `forest auth login` only works one way: username/email + password (with TOTP MFA if enabled), all typed into the terminal.

That hurts in three places:

1. **SSO users have no path.** Operators who registered through Google/GitHub OAuth in forage never set a password, so `forest auth login` refuses them. Their only option is to set a password they don't otherwise need.
2. **MFA in the terminal is brittle.** Pasting TOTP codes into the CLI works but it forces every device-login surface to re-implement the challenge UI. The browser already has the user authenticated against forage with the right cookies, WebAuthn, etc. — we should reuse it.
3. **It's the wrong default for a developer tool in 2026.** `gh auth login`, `gcloud auth login`, `flyctl auth login`, `vercel login` — all default to "open browser, approve device, paste token back." Password-in-terminal is the fallback, not the headline.

The fix: an **OAuth 2.0 Device Authorization Grant** (RFC 8628) flow. CLI opens the browser at the active context's web URL with a short user code. User signs in to forage as they normally would, approves the device, and the CLI receives the same `(access_token, refresh_token)` pair it would have received from password login — written to the same per-context `user-state.json`.

Concrete promise: `forest auth login` becomes a prompt — "Login with [web (recommended) | password]" — and `--web` / `--password` skip the prompt for scripts and CI. Picking *web* opens a browser, displays a code, polls forest-server, and lands you logged in.

## 1. Behavioural contract

### 1.1 New CLI surface

```
forest auth login                # interactive picker; default = web
forest auth login --web          # device-code flow, no prompt
forest auth login --password     # today's username/email + password flow
forest auth login --with-token   # read refresh token from stdin (for CI)
```

Existing flags carry over only under `--password`:

- `--username / --user`, `--email`: only valid with `--password`. Combined with `--web` they error with: *"--username is only valid with --password; web login identifies you in the browser."*

Interactive picker (when no mode flag is passed AND stdin is a TTY):

```
? How would you like to authenticate Forest?
> Login with a web browser   (recommended)
  Login with a password
  Paste an existing refresh token
```

Non-TTY without a mode flag defaults to `--web` and prints "Defaulting to web flow; pass --password for the legacy flow."

### 1.2 Web-flow walkthrough (the happy path the user sees)

```
$ forest auth login
? How would you like to authenticate Forest?  Login with a web browser

! First copy your one-time code: ABCD-EFGH
Press Enter to open https://forage.example.com/device in your browser…

✓ Authentication complete.
  Logged in to context "default" as kasper@understory.io.
```

Behind the scenes:

1. CLI calls `InitiateDeviceLogin` on forest-server. Server returns `{device_code, user_code, verification_uri, verification_uri_complete, expires_in, interval}`.
2. CLI prints `user_code`, copies it to the clipboard if a clipboard provider is available, then opens `verification_uri_complete` (which embeds the user code as a query param so the user usually doesn't need to type anything).
3. Forage `/device` page asks the logged-in user to confirm `user_code` and click **Approve**. Forage backend calls `ApproveDeviceLogin(user_code, on_behalf_of=<user_id>)` on forest-server, which moves the device grant from `pending` → `approved` and attaches the user.
4. CLI polls `PollDeviceLogin(device_code)` every `interval` seconds. While pending: `Status::ResourceExhausted` is *not* used; we use a typed enum (see §1.4) — `pending`, `approved`, `expired`, `denied`, `slow_down`. On `approved`, the response includes `AuthTokens`.
5. CLI writes tokens to `$XDG_DATA_HOME/forest/contexts/<active>/user-state.json` via the existing `UserStateLoader` — zero changes to storage code.

### 1.3 ContextEntry expansion: `web_url`

`ContextEntry` (`apps/forest/crates/forest/src/contexts.rs`) gains one optional field:

```rust
pub struct ContextEntry {
    pub name: String,
    pub server: String,                       // gRPC, unchanged
    pub web_url: Option<String>,              // NEW — forage HTTPS base, e.g. "https://forage.example.com"
    pub created_at: Option<String>,
    pub default_organisation: Option<String>,
}
```

**Resolution order for `web_url` when starting a device flow:**

1. Explicit `ContextEntry.web_url`.
2. `FOREST_WEB_URL` env var (one-shot override; mirrors `FOREST_SERVER`).
3. Convention derivation from `server`: replace the first label `forest` → `forage`, drop port, force `https://`. E.g. `https://forest.dev.understory.sh:443` → `https://forage.dev.understory.sh`. Localhost is a special case: `http://localhost:4040` (forest gRPC) → `http://localhost:3000` (forage's default HTTP port, confirmed in `apps/forage/crates/forage-server/src/main.rs`).
4. If derivation fails (server URL doesn't match a known shape), error with: *"Don't know where to send the browser. Set web_url in the context (`forest context edit <name> --web-url …`) or pass FOREST_WEB_URL."*

`FOREST_PROFILE` install string (per README) gains a `web=` key:

```
FOREST_PROFILE='name=prod,server=https://forest.example.com,web=https://forage.example.com'
```

`forest context create / edit` gain `--web-url <url>` flags. `forest context show` and `forest context list` print it.

### 1.4 New gRPC RPCs (forest-server `UsersService`)

```protobuf
// ─── Device authorisation grant (RFC 8628) ───────────────────────────

service UsersService {
  // … existing RPCs …
  rpc InitiateDeviceLogin(InitiateDeviceLoginRequest) returns (InitiateDeviceLoginResponse);
  rpc PollDeviceLogin(PollDeviceLoginRequest) returns (PollDeviceLoginResponse);
  rpc ApproveDeviceLogin(ApproveDeviceLoginRequest) returns (ApproveDeviceLoginResponse);
  rpc DenyDeviceLogin(DenyDeviceLoginRequest) returns (DenyDeviceLoginResponse);
}

message InitiateDeviceLoginRequest {
  string client_name = 1;        // e.g. "forest-cli/0.3.2 darwin-arm64"
  string client_version = 2;     // semver
  // Optional scopes for future use; ignored today.
  repeated string scopes = 3;
}

message InitiateDeviceLoginResponse {
  string device_code = 1;             // opaque, 256-bit, base64url. CLI keeps; never displayed.
  string user_code = 2;                // human-typeable, e.g. "ABCD-EFGH"
  string verification_uri = 3;         // e.g. "https://forage.example.com/device"
  string verification_uri_complete = 4;// same + "?user_code=ABCD-EFGH"
  int64  expires_in_seconds = 5;       // total TTL (default 900)
  int32  interval_seconds = 6;         // minimum poll interval (default 5)
}

message PollDeviceLoginRequest {
  string device_code = 1;
}

message PollDeviceLoginResponse {
  DeviceLoginStatus status = 1;
  User user = 2;          // populated only when status == APPROVED
  AuthTokens tokens = 3;  // populated only when status == APPROVED
}

enum DeviceLoginStatus {
  DEVICE_LOGIN_STATUS_UNSPECIFIED = 0;
  DEVICE_LOGIN_STATUS_PENDING = 1;     // user has not approved yet
  DEVICE_LOGIN_STATUS_APPROVED = 2;
  DEVICE_LOGIN_STATUS_DENIED = 3;      // user clicked "deny" in forage
  DEVICE_LOGIN_STATUS_EXPIRED = 4;     // past expires_in
  DEVICE_LOGIN_STATUS_SLOW_DOWN = 5;   // CLI polled faster than interval; back off
}

// Called by forage backend after the user clicks Approve in /device.
// Requires service-account bearer auth (same pattern as OAuthLogin).
message ApproveDeviceLoginRequest {
  string user_code = 1;        // looked up case-insensitively, dashes ignored
  string user_id = 2;          // the forage-authenticated user
  string approving_ip = 3;     // forwarded from forage for audit log
  string approving_user_agent = 4;
}

message ApproveDeviceLoginResponse {
  // Empty — CLI gets the tokens via Poll, not this RPC.
}

message DenyDeviceLoginRequest {
  string user_code = 1;
  string user_id = 2;
}
message DenyDeviceLoginResponse {}
```

**Why three RPCs not two (init+poll)?**

The "approve" step lives on forage's backend, not in the CLI, because the user authenticates in the browser against forage. Forage already holds a service-account credential for `OAuthLogin` (see `OAuthLoginRequest` doc) — it reuses the same trust boundary to call `ApproveDeviceLogin` on behalf of the just-authenticated user.

This separation is also what lets us keep `Approve/Deny` privileged (service-account only) while leaving `Initiate/Poll` unauthenticated — they have to be, since the CLI is by definition not yet logged in.

### 1.5 Forage `/device` route

New route: `GET /device` and `POST /device/approve` on forage-server (`apps/forage/crates/forage-server/src/routes/`).

**`GET /device?user_code=ABCD-EFGH`** (user_code optional):

- If not logged in: redirect to `/login?return_to=/device?user_code=…`. After login, return here.
- If logged in: render an "Approve Forest CLI?" page showing:
  - The user code (pre-filled from query string, editable if missing/wrong).
  - The client name/version reported in `InitiateDeviceLogin`.
  - **Approve** and **Cancel** buttons.
  - An advisory: *"Approving this code will let a device sign in to Forest as you. Only approve if you started this from your own machine."*
- **CSRF**: same double-submit cookie pattern the existing OAuth callbacks use (`apps/forage/.../routes/auth.rs`).

**`POST /device/approve`**:

- Body: `{user_code: "...", action: "approve" | "deny"}`.
- Forage backend calls `ApproveDeviceLogin` (or `DenyDeviceLogin`) on forest-server with its service-account credential, passing the authenticated `user_id`, the request IP, and User-Agent.
- On approve: render success page with "You can close this tab and return to your terminal."
- On deny / unknown user_code / expired: render an explanatory error page.

### 1.6 user_code & device_code shape (security-relevant)

- **`device_code`**: 32 random bytes, base64url-encoded (43 chars). Generated by `rand::rngs::OsRng`. Never displayed. Stored hashed (SHA-256) in the DB; comparison is constant-time. Single use — successful `Poll` returning `APPROVED` invalidates it.
- **`user_code`**: 8 chars drawn from a 32-symbol unambiguous alphabet `BCDFGHJKLMNPQRSTVWXZ23456789` (no vowels — to avoid words — and no 0/O/1/I/L). Displayed grouped as `XXXX-XXXX`. Stored uppercased, dashes stripped. ~40 bits of entropy minus DB collision check at issue time.
- **TTL**: `expires_in = 900s` (15 min). `interval = 5s`. Configurable via server config (`apps/forest/crates/forest-server/src/config.rs`).
- **Brute-force defence**: forage backend rate-limits `/device/approve` to N attempts per user_code per minute; after 5 failed lookups for the same authenticated user, lock out for 60s. Failed-attempt counter stored alongside the grant row.
- **One grant per device_code**: forest-server stores grants in a new `device_login_grants` table — `(id, device_code_hash, user_code, client_name, client_version, status, user_id NULL, expires_at, created_at, approved_at NULL, approving_ip NULL, approving_user_agent NULL)`. Index on `user_code` for the approval lookup; index on `device_code_hash` for poll.

### 1.7 Token storage — unchanged

After `Poll` returns `APPROVED` with `AuthTokens`, the CLI writes them through the existing path:

```rust
state.user_state().set_state(&UserState {
    user_id, username, emails,
    access_token: tokens.access_token,
    refresh_access: tokens.refresh_token,
    refresh_after: compute_refresh_after(tokens.expires_in_seconds),
}).await?;
```

No change to file format, file lock, or `UserStateLoader`. The auth middleware (`grpc/interceptor.rs`) doesn't know or care that the tokens came from a device flow.

### 1.8 Status command

`forest auth status` already prints the active context and user. Extend it to also print *how* the session was established when known — `Login method: web (device code, 2026-05-21T14:02Z)` vs `Login method: password`. Stored as a new optional `login_method: Option<String>` field on `UserState`. Backwards-compatible (defaults to `None` = "unknown / pre-feature").

### 1.9 README changes

`forest/README.md` gains:

- A new "## Logging in" section right after "## Install", before "### Shell integration":

  > ```bash
  > forest auth login
  > ```
  >
  > Opens your browser at the active context's forage URL, shows you a short
  > code, and signs you in once you approve. Mirrors `gh auth login`. For
  > scripts or password-based accounts, pass `--password` (or `--with-token`
  > to paste a refresh token directly).

- The `FOREST_PROFILE` example grows a `web=` key.

- A new `FOREST_WEB_URL` row in the "Environment variables" reference (cli.md).

`apps/forest/docs/docs/reference/cli.md` `## forest auth` section gains the new flags. A new walkthrough page `apps/forest/docs/docs/guides/web-login.md` shows the full screenshot-style flow, including what the forage `/device` page looks like.

## 2. Verification architecture (Phase 1b)

### 2.1 Provable properties catalog

| Property | Where | Tool | Why |
|---|---|---|---|
| `user_code` generator never emits a string outside the 32-symbol alphabet | `forest-server` device-login module | `proptest` + Kani (input = RNG seed) | Brute-force model assumes alphabet; ambiguous chars would break that and confuse users |
| `device_code` generator output is uniformly distributed | same | `proptest` statistical | Predictable codes = trivial impersonation |
| State machine of a grant is `pending → {approved, denied, expired}` only — no path back from terminal states | `forest-server` device-login domain | unit + Kani over enum transitions | Re-approval would mint a second token pair from one user consent |
| `device_code` is single-use: `Poll → APPROVED` invalidates the grant | service layer | acceptance test | Replay = persistent unauthorised access |
| `user_code` lookup is case-insensitive and dash-insensitive | same | unit | UX: copy/paste from terminal often loses dashes |
| `Approve` requires service-account bearer auth | grpc handler | acceptance test | Anonymous approval = trivial account takeover |
| Tokens issued via device flow are indistinguishable from tokens issued via password flow (same `user_id`, same `session_id` structure) | service layer | acceptance test | Auth middleware and audit log must treat them the same |
| Polling faster than `interval` returns `SLOW_DOWN`, not tokens | grpc handler | acceptance test | RFC 8628 compliance + DoS defence |
| Expired grants return `EXPIRED`, never `PENDING` or `APPROVED` | service layer | unit (clock-injected) | Time-based bypass |

### 2.2 Purity boundary map

**Pure core (`forest-server/src/domains/device_login.rs`):**

- `DeviceLoginGrant` aggregate: events (`Initiated`, `Approved`, `Denied`, `Expired`), state, command handlers.
- `user_code` and `device_code` generators take a `&mut dyn RngCore` and return strings — no syscalls.
- Status transition function is total, takes `(grant_state, now: DateTime<Utc>, command) -> Result<Event, DomainError>`.

**Effectful shell:**

- `services/device_login_aggregate.rs` — load grant from DB, call domain, `save_with` events + projection updates.
- `grpc/users.rs` — extract actor (none for Initiate/Poll; service-account for Approve/Deny), call service.
- Forage backend `/device` — HTTP, cookies, CSRF, calls `ApproveDeviceLogin` over gRPC.
- CLI clipboard + browser-open (`webbrowser` crate).

**Why this boundary:** the state machine is the part that's easy to get wrong and hard to test through HTTP. Keeping it pure means Kani can enumerate transitions, and proptest can drive thousands of `(state, now, command)` triples in milliseconds. The shell layers are acceptance-tested through the existing `tests/accepttest/` harness.

### 2.3 Verification tooling

- **proptest** for code generator distributions and state-machine sequences.
- **Kani** for the state-transition function (small enough state space).
- **acceptance tests** in `tests/accepttest/` for the full grpc + forage round-trip — including the cross-org isolation check (a user from org A approving cannot mint tokens for a session attributed to org B). Follow `tests/accepttest/authz_flow.rs` pattern (per CLAUDE.md).
- **mutmut / cargo-mutants** on the domain module — any surviving mutation reveals a missing test.

## 3. Edge cases

| # | Scenario | Behaviour |
|---|---|---|
| E1 | User closes browser before approving | CLI keeps polling until `expires_in`, then prints "Code expired. Run `forest auth login` again." Exit code 1. |
| E2 | User approves in browser but CLI was killed | Grant sits in `approved`, never polled. Cleanup job (forest-server) deletes approved-but-unpolled grants after 1h. Tokens were never returned, so nothing leaks. |
| E3 | User types wrong code in `/device` | Forage shows "No such code." Counter increments. Five wrong codes from the same user → 60s lockout for that user (not for the device, since the device may not even exist). |
| E4 | Two CLIs initiated, user approves one | Each has its own device_code. Only the matching one returns tokens. The other expires normally. |
| E5 | CLI polls faster than `interval` | Server returns `SLOW_DOWN`. CLI doubles its interval (capped at 30s). |
| E6 | Network drops mid-poll | CLI retries with exponential backoff inside the interval; gives up after `expires_in`. |
| E7 | `web_url` not derivable and not set | CLI errors before opening anything; suggests `forest context edit --web-url …`. |
| E8 | User on a headless box (no browser) | `--web` prints the URL and the user_code; user opens the URL on another device. Same flow works because the user_code travels independently. |
| E9 | Browser opens but JavaScript disabled in forage | The approve button is a plain `<form method=POST>`. Works without JS. |
| E10 | Context switched mid-login (`FOREST_CONTEXT=other` set after CLI started) | Tokens are written to the context that was active when the CLI resolved on startup, not the one in the env at write time. Already true of password login; documented. |
| E11 | Server's clock is skewed from CLI's | All timestamps are server-side; CLI doesn't enforce TTLs locally. |
| E12 | User has MFA enabled | MFA challenge happens in forage browser flow, not in CLI. CLI sees only `PENDING → APPROVED`. This is the primary UX win. |
| E13 | User's password account exists; they've never used OAuth | Web flow still works — they sign into forage with username+password+MFA in the browser, then approve. The device grant is independent of which forage-side auth method they used. |
| E14 | CLI is invoked with `--web` against a server that doesn't implement the new RPCs (older deployment) | gRPC returns `Unimplemented`; CLI prints "This forest server doesn't support web login. Pass --password or upgrade the server." Exit 1. |
| E15 | `forest auth status` after device login on a pre-`login_method` token | Prints `Login method: unknown` rather than failing. |

## 4. Migration & backward compatibility

- **ContextEntry**: `web_url` is optional. Existing `contexts.json` files load unchanged. First call that needs `web_url` derives or errors per §1.3.
- **UserState**: `login_method` is optional. Existing `user-state.json` files load unchanged.
- **CLI default**: changing the default from "prompt for password" to "prompt for method, default web" is technically a breaking change for users who scripted `forest auth login --username foo` and piped a password to stdin. We mitigate by:
  - `--password` continues to behave exactly as today (no prompt, reads `FOREST_PASSWORD`).
  - The release note calls out the change.
  - Non-TTY without a mode flag defaults to `--web` *and* prints a deprecation hint about scripting against the new flag.
- **Server**: new RPCs are additive. Old CLIs keep working against the new server unchanged.
- **DB migration**: one new table `device_login_grants` and a cleanup background job. Standard `sqlx migrate` (per CLAUDE.md, `sqlx migrate run --source crates/forest-server/migrations`, then `cargo sqlx prepare`).

## 5. Out of scope (for this task)

- **Per-scope authorisation** (`scopes` field in `InitiateDeviceLoginRequest`). Stored for future use; ignored today.
- **Personal access tokens** (`forest auth token` already exists for non-interactive CI; not changing it).
- **WebAuthn at the CLI level.** The forage side already handles platform authenticators if configured; the CLI sees only the device-grant result.
- **Replacing password login.** Password remains a first-class supported mode; this task only adds web as a peer.
- **CLI-side encryption of stored tokens.** Orthogonal; tracked separately.

## 6. Open questions (resolve before Phase 2)

1. **Localhost convention port.** ~~Is forage's local dev port really `4041`?~~ Resolved: forage defaults to `PORT=3000` (`apps/forage/crates/forage-server/src/main.rs:81-84`). Spec updated.
2. **Clipboard.** Do we want a dependency on `arboard`/`copypasta` just for the "copied!" UX, or accept that the user reads + types? Lean: skip clipboard for v1, add later if asked.
3. **Should `ApproveDeviceLogin` reuse the existing service-account credential forage already holds for `OAuthLogin`, or do we issue a new narrower one?** Reusing means one fewer secret to rotate; issuing a new one means a smaller blast radius if leaked. Recommend reuse for v1; tracked.
4. **`forest auth login --web` from inside a tmux/ssh session on a headless host:** webbrowser crate will try `xdg-open` and fail silently. We must detect that (the crate returns a Result) and fall through to "open this URL manually" rather than hang waiting for approval the user can't give.
5. **Telemetry.** Should `InitiateDeviceLogin` accept an opaque `client_telemetry_id` so we can correlate "code issued" with "code polled" in metrics? Adds value, adds privacy surface. Defer to a follow-up.

## 7. Chainlink bead skeleton (for Phase 2 task breakdown)

- 022.1 Domain: `DeviceLoginGrant` aggregate (events, state, transitions) — pure core
- 022.2 Code generators with property tests
- 022.3 DB migration + projection
- 022.4 gRPC: `InitiateDeviceLogin` + `PollDeviceLogin` (unauthenticated)
- 022.5 gRPC: `ApproveDeviceLogin` + `DenyDeviceLogin` (service-account)
- 022.6 ContextEntry `web_url` field + resolution + `forest context edit --web-url`
- 022.7 CLI: `--web` flag + interactive picker + browser opener + polling loop
- 022.8 CLI: `UserState.login_method` field + `forest auth status` update
- 022.9 Forage `/device` route (GET + POST) with CSRF
- 022.10 README + cli.md + new guide page
- 022.11 Acceptance test: full round-trip (CLI → forage → server → CLI)
- 022.12 Acceptance test: cross-org isolation, expired, denied, slow-down, replay
- 022.13 Kani harness on state transitions
- 022.14 Cleanup job for stale grants

## 8. Implementation status

Shipped in this slice:

- **022.1 Domain aggregate** — `crates/forest-server/src/domains/device_login.rs`. 32 unit tests covering state-machine, TTL, replay-masking, hydration, serde.
- **022.2 Code generators** — same file. Tests cover alphabet conformance, distribution, URL-safety, uniqueness, hash determinism, normalization.
- **022.3 DB migration** — `migrations/20260521000000_device_login_grants.sql`. Unique indexes on `device_code_hash` and `user_code`, partial index on `expires_at` for the sweep job.
- **022.4 InitiateDeviceLogin + PollDeviceLogin** — unauthenticated per RFC 8628, on the `auth_layer.rs` whitelist. PollDeviceLogin masks `Consumed` and "unknown code" as `EXPIRED`.
- **022.5 ApproveDeviceLogin + DenyDeviceLogin** — service-account-only via `require_service_account()`; covered by acceptance tests for both the happy path and the unauth-rejection path.
- **Security defences in place at the server**:
  - Hard input bounds on `client_name`, `client_version`, `scopes`, `device_code`, `user_code`.
  - Oversize `device_code` returns `EXPIRED` (anti-enumeration).
  - SHA-256 hash of `device_code` stored; raw value never persisted.
  - Single-use enforcement is atomic with the event-store transaction.
  - Slow-down gating only applies to `Pending` grants — terminal states are returned uniformly to defeat replay-detection-by-timing.
  - Audit logging on every approve/deny: approver `user_id`, IP, UA, code prefix (2 chars), client name+version. Full `user_code` and `device_code` never logged.
  - 14 acceptance tests including anti-replay, cross-user re-approval guard, unauthed-approve rejection, oversized-input rejection, user_code normalisation.
- **Config** — `web_app_url: Option<String>` on `Config`, sourced from `FOREST_WEB_APP_URL`. `Initiate` errors when unconfigured.
- **Authz coverage test** — Initiate/Poll added to EXEMPT with RFC 8628 reason; Approve/Deny pass the standard `require_service_account` gate.

Also shipped:

- **022.6** ContextEntry `web_url` field — `crates/forest/src/contexts.rs`. Resolution chain: explicit field → `FOREST_WEB_URL` env → convention (`forest.X → forage.X`, `localhost:4040 → localhost:3000`) → None. 8 new unit tests. New `forest context set-web-url <name> <url>` / `--clear` subcommand. `forest context provision` gains `--web-url`. `scripts/install.sh` parses `web=` from `FOREST_PROFILE` and forwards it through.
- **022.7** CLI flow — `crates/forest/src/cli/auth/login.rs` + new `login_web.rs`. Interactive picker (web recommended) when stdin is a TTY; non-TTY defaults to web with a deprecation hint. `--web` / `--password` flags skip the prompt. `--username` / `--email` only valid with `--password`. Polls server with `interval_seconds`, honours `SlowDown` by widening the interval (+5s, cap 30s). Browser opens with `webbrowser` crate; failure prints the URL for manual entry. Tokens written through the existing `UserStateLoader` — storage unchanged.

Pending:

- **022.8** `forest auth status` printing login method. Cosmetic.
- **022.9** Forage `/device` route — GET form + POST handler calling ApproveDeviceLogin / DenyDeviceLogin via the existing service-account credential. **Required for the feature to work end-to-end** but in a different app (`apps/forage`) so naturally a follow-up PR.
- **022.10** README + cli.md + `docs/guides/web-login.md`.
- **022.11–14** Deferred: Kani harness on state transitions, sweep job for stale grants, end-to-end CLI↔forage↔server round-trip test.
- **Open**: per-IP rate limit on `Initiate` (currently relies on forage-layer rate limits + the slow-down poll defence). Flagged in §6.
