# 019: Email verification flow at signup

> Status: **SPEC — awaiting human review.** Per `../../spec.md`, no tests
> or implementation until this contract is signed off.
>
> Companion to `018-registration-email-domain-restriction.md`. The flag
> landed there (`FOREST_REQUIRE_EMAIL_VERIFICATION`) is the precondition
> that this task makes meaningful by adding an actual verification flow.

## Intent

When `FOREST_REQUIRE_EMAIL_VERIFICATION=true`, native signup must not
trust the address until the user proves they own it. This task adds:

1. A token-based email verification flow that piggy-backs on forage's
   existing magic-link infrastructure (token table, NATS+SMTP pipeline,
   templates).
2. The runtime gates: native `register` no longer auto-issues tokens
   when verification is required; `login` is blocked until at least one
   of the user's emails is verified.
3. OAuth signups bypass verification (provider already verified) and now
   correctly mark the email row as `verified = true` at creation time —
   today they don't, which is a latent bug.

## Architectural split

- **Forest** stays mailer-free. It owns: the truth of `verified`, the
  config gate, the proto contract, the `verify_email` RPC.
- **Forage** owns: token issuance/storage, SMTP send via NATS, the
  email templates, the public verify URL, the "check your inbox" UX.
  This is where the verification-flow code actually lives.

## Phase 1a — Behavioral Contract (Forest)

### Config & gates (already in 018)

`FOREST_REQUIRE_EMAIL_VERIFICATION=true` flips on the runtime gates
specified below. With the flag off, behavior is identical to today.

### `register` (gRPC)

| Flag | Behavior |
|---|---|
| `require_email_verification = false` | Today's behavior: insert user, issue tokens, log in. |
| `require_email_verification = true` | Insert user (transactionally as today), do **not** issue tokens. Return `RegisterResponse { user, tokens: None, email_verification_required: true }`. |

A new proto field on `RegisterResponse`:

```proto
message RegisterResponse {
  User user = 1;
  AuthTokens tokens = 2;          // empty when verification required
  bool email_verification_required = 3;
}
```

Backwards-compat: when the flag is off, the new field is `false` and
`tokens` is populated as today. Forage clients that don't read the new
field continue to work.

### `login_by_username` / `login_by_email` (service layer) and `Login` (gRPC)

When `require_email_verification = true`, password verification succeeds
but the gRPC handler then checks: does the user have **any** verified
email? If no, return `tonic::Status::failed_precondition` with the
canonical detail string `"email_not_verified"`. This lets forage detect
the case and render a "resend verification" page without parsing the
human-readable message.

OAuth login (`o_auth_login`) is unaffected — provider-supplied emails
are marked verified at creation (see below).

### `register_oauth_user` (service layer)

Bug fix: the email row created during OAuth signup must have
`verified = true`. Today `add_user_email` defaults `verified = false`,
so OAuth-only users have unverified emails forever. Add a
`add_user_email_verified(user_id, email)` repository method (or a
parameter on the existing one) and use it from `register_oauth_user`.

This is correct regardless of the verification flag — the OAuth
provider has already verified the email, so the row should reflect that.
Without this fix, the login-block above would lock OAuth users out the
moment the flag flips on, which is wrong.

### `verify_email` (gRPC) — authorization expansion

Today: requires a user JWT (per `auth_layer.rs`).

Add: also accepts an `Actor::ServiceAccount` (as `o_auth_login` does).
This lets forage redeem a verification token and call `verify_email`
on behalf of a user who is not yet logged in. The user-JWT path is
preserved for clients that want to re-verify an email manually.

The handler must enforce: if the actor is a user, `actor.user_id ==
req.user_id` (no cross-user verification by JWT). Service accounts can
verify any user (cross-org infra access pattern).

### Email-domain regex (interaction with 018)

When `FOREST_REGISTRATION_EMAIL_DOMAIN_REGEX` is set, `register` still
checks the regex *before* this task's flow kicks in (so a disallowed
domain is rejected before a verification email is queued). No change
required from 018; the existing `enforce_registration_policy` call site
in `register` is correctly upstream of the new "do not issue tokens"
branch.

## Phase 1a — Behavioral Contract (Forage)

### DB migration

Extend the existing `magic_link_tokens` table — do not introduce a
parallel verification-tokens table. The single-use, hashed-token, TTL,
rate-limit primitives are identical; the only difference is what the
token unlocks.

```sql
ALTER TABLE magic_link_tokens
    ADD COLUMN token_type TEXT NOT NULL DEFAULT 'magic-link';

-- Replace the email-only index with a (token_type, email) one for
-- per-type rate-limit counts.
DROP INDEX IF EXISTS idx_magic_link_tokens_email;
CREATE INDEX idx_magic_link_tokens_type_email
    ON magic_link_tokens (token_type, email);
```

Default value is `'magic-link'` so existing rows back-fill correctly.
The PK stays `token_hash` (raw token bytes are wide enough that
collisions across types are statistically impossible — but the
verify-and-consume now also compares `token_type` to be belt-and-braces).

### `MagicLinkStore` trait change

```rust
#[async_trait::async_trait]
pub trait MagicLinkStore: Send + Sync {
    async fn store_token(
        &self,
        token_type: &str,           // NEW
        token_hash: &str,
        email: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), MagicLinkError>;

    async fn verify_and_consume(
        &self,
        token_type: &str,           // NEW
        token_hash: &str,
    ) -> Result<Option<String>, MagicLinkError>;

    async fn count_recent(
        &self,
        token_type: &str,           // NEW
        email: &str,
        since: DateTime<Utc>,
    ) -> Result<u64, MagicLinkError>;

    async fn reap_expired(&self) -> Result<u64, MagicLinkError>;
}
```

Constants for the two known types live in `forage_core::auth::magic_link`:

```rust
pub const TOKEN_TYPE_MAGIC_LINK: &str = "magic-link";
pub const TOKEN_TYPE_EMAIL_VERIFY: &str = "email-verify";
```

All call sites in the existing magic-link login code pass
`TOKEN_TYPE_MAGIC_LINK`. New call sites pass `TOKEN_TYPE_EMAIL_VERIFY`.

### New routes (forage)

```
GET  /auth/verify-email                — token-redemption page (?token=...)
POST /auth/verify-email/resend         — re-send the verification email
```

#### `GET /auth/verify-email?token=xxx`

1. SHA-256 the raw token.
2. `magic_link_store.verify_and_consume(TOKEN_TYPE_EMAIL_VERIFY, hash)`.
3. If `None` (expired or already used): render an error page with a
   "request a new link" CTA pointing at `/auth/verify-email/resend`.
4. If `Some(email)`: look up the user by email via
   `forest_client.get_user_by_email(email)` (service-account auth).
5. Call `forest_client.verify_email(user_id, email)` (service-account
   auth — see RPC change above).
6. Render a success page with a "log in now" link. **Do not** create a
   session here — the user must enter their password to log in. This
   keeps the verification flow incapable of session-hijacking even if
   the email is intercepted.

#### `POST /auth/verify-email/resend`

Form takes `email`. Validates format. Calls
`count_recent(TOKEN_TYPE_EMAIL_VERIFY, email, now - 15min)`; if ≥3,
silently shows the "check your inbox" page (rate-limit without leaking
that the email is registered). Otherwise: generate token, enqueue
email, render the same "check your inbox" page.

### Signup handler change (forage)

In `signup_submit` (`apps/forage/crates/forage-server/src/routes/auth.rs:93`),
after a successful `forest_client.register(...)` call:

- If response has `tokens` and `!email_verification_required`: today's
  behavior (create session, redirect to dashboard).
- If `email_verification_required` is true (so `tokens` is empty):
  trigger the verification email and render the "check your inbox" page.
  Do **not** create a session.

The verification trigger is a forage-internal helper:
`fn enqueue_verification_email(state: &AppState, email: &str)` that:
1. Generates a fresh token via `generate_magic_link_token()`.
2. Stores it via `magic_link_store.store_token(TOKEN_TYPE_EMAIL_VERIFY, hash, email, now+15min)`.
3. Builds an `EmailEnvelope { email_type: "email-verify", to: email, … }`
   with subject "Verify your email" and a body containing the link
   `{external_host}/auth/verify-email?token={raw}`.
4. Publishes to JetStream subject `forage.email.email-verify`.

The existing `EmailConsumer` already routes by subject prefix
(`forage.email.>`), so no consumer change is needed.

### Login handler change (forage)

`login_submit` already calls `forest_client.login(...)`. When forest
returns `failed_precondition` with detail `"email_not_verified"`, render
a page that says "Please verify your email" with the same resend form
above (pre-filled with the typed email).

### Templates

New jinja templates under `apps/forage/templates/pages/`:
- `verify_email_check_inbox.html.jinja` — shown after signup or resend.
- `verify_email_success.html.jinja` — shown after successful redemption.
- `verify_email_failed.html.jinja` — for expired/used tokens.
- `email_verification_email.html.jinja` and `…_text.txt.jinja` — the
  email body itself (HTML + plaintext, mirroring magic-link).

## Phase 1a — Edge Case Catalog

| # | Scenario | Expected behavior |
|---|---|---|
| 1 | Verification flag off, native signup | Today's behavior, tokens issued. |
| 2 | Verification flag on, native signup | User created, no tokens, "check your inbox" page, email sent. |
| 3 | Click verification link | Email marked verified; success page with "log in" link; **no session created**. |
| 4 | Click link a second time | "link expired or already used" page. |
| 5 | Click link after 15 min | Same as 4. |
| 6 | Tampered token (random string) | Same as 4 — never leak whether the token existed. |
| 7 | Resend request, ≤3 in last 15 min | New token issued, email sent, "check your inbox" page. |
| 8 | Resend request, >3 in last 15 min | "check your inbox" page (no error, no leak). |
| 9 | Login with unverified email, flag on | gRPC `failed_precondition: email_not_verified`; forage shows "verify email" page with resend form. |
| 10 | Login with verified email, flag on | Today's login behavior. |
| 11 | Login flag on, user has multiple emails, one verified | Allowed (any verified email is sufficient). |
| 12 | OAuth signup, flag on | User created, email row inserted with `verified = true`, tokens issued, no verification email sent. |
| 13 | OAuth login for an OAuth user created *before* this task lands | Email row may be `verified = false` historically — login blocked when flag is on. Mitigated by a backfill migration (see below). |
| 14 | Verification email send fails (SMTP down) | Signup still succeeds (user row exists). Forage logs the failure. User can hit `/auth/verify-email/resend` once SMTP recovers. |
| 15 | Token verified, but `verify_email` RPC fails (network blip) | Forage shows a generic "try again later" page; the token has already been consumed (single-use), so the user has to request a new one. **Recovery path must exist** — see "Open question" below. |
| 16 | Two concurrent clicks on the same link | One wins (DELETE-RETURNING is atomic); the other gets the "already used" page. |
| 17 | User registers, never clicks, registers again with same email | Second `register` fails with the existing `unique_violation` on `user_emails.email`. (Pre-existing behavior, not changed here.) |
| 18 | Domain regex enabled (018) and verification flag enabled (019) | Domain regex check runs first; rejects disallowed domains *before* any token is issued. |
| 19 | Direct gRPC client (not forage) calls `register` with flag on | Same as 2: response signals `email_verification_required`. Caller must handle the absence of tokens. Documented in proto. |

### Backfill for edge case 13

A one-time migration on forest:

```sql
-- All emails belonging to users who have an OAuth identity but no
-- native identity should be considered verified — the OAuth provider
-- vouched for them at signup time.
UPDATE user_emails ue SET verified = true
WHERE NOT EXISTS (
    SELECT 1 FROM identities i
    WHERE i.user_id = ue.user_id AND i.provider = 'native'
)
AND verified = false;
```

Ordering is important: this migration must land in the same release as
the `register_oauth_user` fix and the login-block. Otherwise existing
OAuth users get locked out the moment the flag flips on.

## Phase 1a — Non-Functional Requirements

- **Security.**
  - Tokens are 32 bytes of CSPRNG, base64url, single-use, 15-min TTL.
    (Reuses the existing `generate_magic_link_token` primitive verbatim.)
  - Token hash is SHA-256 hex; only the hash is in the DB.
  - Verification success **does not** create a session. The redemption
    path can't be turned into a login bypass.
  - Service-account auth on `verify_email` is gated by the existing
    `Actor::ServiceAccount` extension extraction in forest's auth layer.
    No new auth surface.
  - Rate-limit: 3 sends per email per 15 minutes (per type — magic-link
    and email-verify counters are independent so a malicious actor
    can't lock a victim out of one by spamming the other).
- **Privacy.**
  - The email body must include only the verification link and a
    short instruction. No marketing, no other PII.
  - Resend rate-limit must not leak whether an email is registered
    (always show "check your inbox").
- **Performance.**
  - One INSERT + one publish per signup. No additional DB roundtrips
    on the hot path of registration.
- **Observability.**
  - Forage emits a counter on every send + every redemption (success /
    expired / used). Forest emits a tracing span on each `verify_email`
    call with the `actor_type` (user vs service_account).
- **Backwards compatibility.**
  - With the flag off, native register, OAuth login, and login behave
    identically to today (modulo the OAuth `verified=true` fix, which
    is correct independent of the flag).
  - The new proto field defaults to `false` so old clients see the
    same shape they always did.

## Phase 1b — Verification Architecture

**Pure / effectful split:**

- **Forage pure core (`forage-core`):**
  - `magic_link::generate_magic_link_token`, `hash_magic_link_token`,
    constants — already pure, no change.
  - `MagicLinkStore` trait — pure interface.
- **Forage effectful shell (`forage-server`):**
  - `enqueue_verification_email` (DB write + NATS publish).
  - The two new HTTP routes.
  - The signup-handler branch and the login-handler branch.
- **Forest pure core:**
  - The gate logic on `register` (does the user-facing response include
    tokens?) is a function of `Config::require_email_verification` and
    is trivially testable.
  - The "any verified email?" predicate on the user's email list is
    pure and table-driven.
- **Forest effectful shell:**
  - The expanded auth check on `verify_email`.
  - The `register_oauth_user` change to mark the email verified.
  - The backfill migration.

**Provable properties (test-level):**

- ∀ user, ∀ flag, `register(flag=false)` returns a session ⇔
  `register(flag=true)` returns `email_verification_required=true` and
  no tokens.
- ∀ user, ∀ flag, `login(flag=true, no_verified_email)` returns
  `failed_precondition: email_not_verified`.
- Token redemption is single-use: a successful `verify_and_consume`
  followed by a second `verify_and_consume` of the same hash returns
  `None` (already enforced for magic-link; the new column doesn't
  weaken it).
- A token of `token_type='magic-link'` cannot be redeemed at the
  email-verify route (and vice versa) — the `token_type` is part of
  the lookup key.

## Phase 2 — Test plan (to be written in Phase 2a, not now)

### Forest unit tests

- `services/users.rs::register` with flag on returns `tokens=None,
  email_verification_required=true`.
- `services/users.rs::register` with flag off returns
  `email_verification_required=false`.
- `services/users.rs::register_oauth_user` inserts the email row with
  `verified=true`.
- `login_by_*` returns `Err(NotVerified)` (a new sentinel error from
  the service layer) when flag is on and no email is verified.

### Forest gRPC acceptance tests (`tests/accepttest/email_verification.rs`)

Build a third fixture variant that has both `domain_regex = Some(@understory\.io$)`
and `require_email_verification = true`. Tests:

- Native register: `tokens=None`, `email_verification_required=true`,
  user row exists, email row has `verified=false`.
- Login with unverified email: `failed_precondition: email_not_verified`.
- After flipping `verified=true` directly via the repo (simulating
  forage's verify-email roundtrip), login succeeds.
- OAuth signup (service-account auth): email row inserted with
  `verified=true`, login allowed without verification.
- `verify_email` RPC with service-account auth marks `verified=true`.
- `verify_email` RPC with user JWT for a *different* user is rejected
  `permission_denied` (cross-user JWT verification is not allowed).
- The 018 domain regex still applies and runs first.

### Forage unit + route tests

- `MagicLinkStore` impls (in-memory + Postgres) honor the new
  `token_type` parameter; cross-type tokens cannot be redeemed.
- Migration rolls forward and back cleanly on a fresh DB.
- `signup_submit` with `email_verification_required=true` enqueues an
  envelope with `email_type=email-verify`, does not create a session,
  and renders the "check your inbox" template.
- `/auth/verify-email?token=…` happy path: redeems token, calls forest
  `verify_email`, renders success.
- `/auth/verify-email?token=expired` renders the failure page.
- `/auth/verify-email/resend` with ≤3 sends produces a new token; with
  ≥3 silently 200s without enqueueing.
- `login_submit` returning `email_not_verified` from forest renders the
  resend form with the email pre-filled.

## Phase 3 — Adversarial review checklist (filled in after impl)

Likely Adversary hits to pre-empt:

- Does the verification flow create a session? **Spec says no.** Confirm
  in implementation.
- Does the resend rate-limit leak whether an email is registered?
  **Spec says no.** Confirm: same response shown for unknown emails,
  unrate-limited, and rate-limited.
- Is the verification link routable from a phishing context (e.g.
  embedded image)? Forage should set `Referrer-Policy: no-referrer` on
  the verify-email route so the raw token never leaks via Referer.
- Is the OAuth backfill migration idempotent? (Multiple deploy cycles.)
- Does `verify_email` short-circuit when the email is already verified?
  Yes — repository UPDATE with `WHERE verified = false` makes the call
  a no-op on the second visit. Cheaper, and avoids spurious "modified
  N rows" surprises in tests.
- Concurrency: two simultaneous redemptions of the same token. The
  `DELETE … RETURNING` pattern in `verify_and_consume` is atomic; only
  one wins. Add an explicit test.
- The email body URL — does it use `external_host` (HTTPS in prod) or
  the request's host header (spoofable)? Must be `external_host` from
  config.

## Resolved decisions

1. **Edge case 15: single-use is absolute.** Token is consumed before
   forest is called; if the verify-email RPC fails, the user requests a
   new link. A page refresh after a successful redemption hits the
   "already used" branch as designed.

2. **Two RPCs, not dual-auth.** `verify_email` stays user-self only
   (user JWT, `actor.user_id == req.user_id`). A new RPC
   `ConfirmEmailVerification(user_id, email)` is added, gated on
   `Actor::ServiceAccount` only. Forage calls this one after redeeming
   a verification token. Justification: service accounts are machine
   identities without their own email and shouldn't be conflated with
   "user verifying their own email" — separate concerns get separate
   RPCs.

3. **Backfill migration ships in this release.** The OAuth-user
   `verified=true` backfill goes in the same migration set as the
   `register_oauth_user` fix and the runtime gates. The flag flip
   itself happens in a follow-up deploy after monitoring confirms no
   OAuth user got locked out.

4. **`add_email` verification is in scope.** When
   `require_email_verification = true` and a logged-in user adds a
   new email, the new email is created `verified = false`, and forest
   signals `email_verification_required = true` on the response so
   forage can trigger the verification flow for that address.

5. **Template copy.** Forage will reuse the magic-link template idiom
   (header, single CTA button, expiry note). Variables exposed to the
   template: `user_email`, `verification_url`, `ttl_minutes` (=15).
   Subject line: `"Verify your email for Forage"`. Plaintext fallback
   reads "Click the link below to verify your email: {verification_url}
   — link expires in {ttl_minutes} minutes."

## Additional scope note

Driven by decision 4, the proto contract for `AddEmailResponse` also
gains an `email_verification_required` field (mirroring
`RegisterResponse`). When forage's `add_email` route sees that flag
true, it enqueues a verification email for that specific address using
the same primitive as signup.
