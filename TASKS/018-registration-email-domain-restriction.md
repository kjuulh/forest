# 018: Registration email-domain restriction

> Status: **SPEC — awaiting human review.** Per `../../spec.md`, no tests or
> implementation until this contract is signed off.

## Intent

Allow operators to disable open registration on a Forest instance and only
permit accounts whose primary email matches an operator-supplied regex (e.g.
`@understory\.io$`). When the domain restriction is on, native registration
must also have email verification enforced — otherwise an attacker could just
claim `victim@understory.io` and take the slot.

## Phase 1a — Behavioral Contract

### Configuration surface

Two new entries on `state::Config` (and matching CLI/env flags wired in
`cli.rs`):

| Field | Env var | Type | Default |
|---|---|---|---|
| `registration_email_domain_regex` | `FOREST_REGISTRATION_EMAIL_DOMAIN_REGEX` | `Option<regex::Regex>` | `None` |
| `require_email_verification` | `FOREST_REQUIRE_EMAIL_VERIFICATION` | `bool` | `false` |

`registration_email_domain_regex` is parsed and compiled at startup. The raw
string is **not** stored beyond compilation; only the compiled `Regex` lives
on `Config` so we don't risk re-parsing on every request.

### Startup invariants (checked once, before serving)

1. **Regex compiles.** If the env var is set but cannot be parsed by the
   `regex` crate, startup fails with a clear error naming the env var.
2. **Verification is enforced when the gate is on.** If
   `registration_email_domain_regex` is `Some(_)` *and*
   `require_email_verification` is `false`, startup fails with:
   `"FOREST_REGISTRATION_EMAIL_DOMAIN_REGEX is set but FOREST_REQUIRE_EMAIL_VERIFICATION is false; native registration would let attackers claim emails they don't own"`.
3. **OAuth signup is exempt from invariant (2).** OAuth identities arrive
   pre-verified by the provider (Forage gates this with service-account auth
   in `o_auth_login`). So the verification-required check applies *only* to
   the native code path; the same domain regex does still gate OAuth signup
   (see runtime invariant 5 below).

### Runtime invariants (per request)

1. **Native register matches regex.** `UsersService::register` compares
   `req.email` against the compiled regex. On miss: return
   `tonic::Status::permission_denied("registration is restricted to allowed email domains")`.
   No partial side effects: the user row, identity, credential, and email
   are all created in a single transaction (already true today) so a regex
   miss must short-circuit *before* `repo.begin()`.
2. **Native register requires verification flow to exist.** Implementation
   (B) — see "Open question" below — relies on the startup gate for now;
   no per-request behavioral change beyond what invariant 1 covers.
3. **OAuth signup matches regex.** In `o_auth_login`, the regex is applied
   to `req.provider_email` *only on the new-user branch* (where we'd call
   `register_oauth_user`). If the email belongs to an existing user already
   (the link-provider branch) or the OAuth identity is already known, the
   regex is not re-checked — those users were already admitted and we don't
   want to lock them out retroactively.
4. **OAuth login (existing identity) is never blocked by the regex.** Same
   reasoning as 3; logins for existing accounts must keep working even if
   the operator tightens the regex later.
5. **`add_email` is also gated by the regex.** Otherwise a user signs up
   under `kasper@understory.io`, then adds `attacker@anywhere.com` and uses
   that to log in. This must be checked in
   `UsersService::add_email` / `services::users::UserService::add_email`.
   Returns the same `permission_denied` error.

### Out of scope for this task

- Token-based email verification flow (sending the email, the token table,
  the `confirm_email_with_token` RPC). Tracked separately. This task only
  introduces the *flag* that says "verification is required" and uses it as
  a startup precondition. The actual mailer is left for a follow-up.
- A UI/admin endpoint for editing the regex at runtime. The regex is set at
  process start and applies until restart.
- Per-organisation regex overrides. The regex is global to the Forest
  instance.

## Phase 1a — Interface Definition

### Pure core

A small, deterministic function in a new module
`crates/forest-server/src/services/registration_policy.rs`:

```rust
pub struct RegistrationPolicy {
    domain_regex: Option<regex::Regex>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistrationPolicyError {
    #[error("email does not match the allowed domain pattern")]
    DomainNotAllowed,
}

impl RegistrationPolicy {
    pub fn new(domain_regex: Option<regex::Regex>) -> Self;

    /// Returns `Ok(())` when the email is allowed to register, or
    /// `Err(DomainNotAllowed)` when it isn't. When no regex is configured
    /// every email is allowed.
    pub fn check_email(&self, email: &str) -> Result<(), RegistrationPolicyError>;
}
```

This is the pure, testable core. No I/O, no state, no clock. The regex is
applied with `Regex::is_match` against the trimmed lower-cased email.
Lower-casing is a deliberate decision: email local parts are technically
case-sensitive per RFC 5321 but every real-world provider treats them
case-insensitively, and the operator-supplied regex should not have to
duplicate `[Aa][Bb]…` patterns.

### Effectful shell

- `Config` gains the two new fields. `cli.rs` parses the env vars and
  compiles the regex. The startup invariant check (regex set ⇒ verification
  required) lives in `cli::execute` *after* `Config` is built but *before*
  `State::new` so we never even open a DB connection on a misconfigured
  instance.
- `State` gets a `registration_policy(&self) -> RegistrationPolicy` accessor
  so call sites don't reach into `config` directly.
- `UsersServer::register`, `UsersServer::o_auth_login` (new-user branch),
  and `UsersServer::add_email` consult `state.registration_policy()` and
  map `RegistrationPolicyError::DomainNotAllowed` to
  `tonic::Status::permission_denied`.

## Phase 1a — Edge Case Catalog

| # | Input | Expected behavior |
|---|---|---|
| 1 | regex unset, any email | allowed (backwards-compatible) |
| 2 | regex `@understory\.io$`, email `kasper@understory.io` | allowed |
| 3 | regex `@understory\.io$`, email `attacker@evil.com` | `permission_denied` |
| 4 | regex `@understory\.io$`, email `Kasper@Understory.IO` | allowed (case-insensitive normalization) |
| 5 | regex `@understory\.io$`, email `kasper@understory.io.evil.com` | rejected (anchor `$` matters; spec authors must terminate with `$`) |
| 6 | regex `@understory\.io$`, email `   kasper@understory.io   ` | allowed (whitespace trimmed before match) |
| 7 | regex `@understory\.io$`, email `""` | rejected |
| 8 | regex `@understory\.io$`, OAuth `o_auth_login` for *existing* known identity with non-matching email | allowed (login path, not signup path) |
| 9 | regex `@understory\.io$`, OAuth `o_auth_login` for *new* user with non-matching email | rejected with `permission_denied` |
| 10 | regex `@understory\.io$`, `add_email` with non-matching email on an existing account | rejected with `permission_denied` |
| 11 | regex set, `FOREST_REQUIRE_EMAIL_VERIFICATION=false` | startup error |
| 12 | invalid regex `[unclosed` | startup error citing `FOREST_REGISTRATION_EMAIL_DOMAIN_REGEX` |
| 13 | regex unset, `FOREST_REQUIRE_EMAIL_VERIFICATION=true` | OK at startup; verification flag is independently honored once that flow lands |
| 14 | regex `^$` (matches only empty string) | technically allowed configuration; every real email rejected |

## Phase 1a — Non-Functional Requirements

- **Performance.** Regex match is O(email length × regex size); negligible.
  Compile *once* at startup, share via `Arc` inside `RegistrationPolicy` so
  the `RegistrationPolicy` accessor on `State` is cheap.
- **Security.**
  - Reject before any DB write.
  - Constant-ish timing is **not** a requirement — registration is not a
    secret-comparison endpoint; an attacker probing the regex by trial is
    the same as an attacker probing the email-already-exists check, which
    we already accept.
  - The error returned to the client is a generic
    `permission_denied("registration is restricted to allowed email domains")`.
    We do **not** echo the regex or hint at the allowed domain — that's
    operator information, not user information.
- **Observability.** A `tracing::warn!` on each rejection with the email's
  domain only (everything after the last `@`), not the full address, to
  help operators tune the regex without leaking PII into logs.
- **Backwards compatibility.** When neither env var is set, behavior is
  identical to today's. No migration required.

## Phase 1b — Verification Architecture

**Pure core / effectful shell split.**

- Pure core: `RegistrationPolicy` (file `services/registration_policy.rs`).
  No `async`, no DB, no clock, no global state. Trivially fuzzable.
- Effectful shell: the three gRPC handlers, the `Config`/CLI wiring, and
  the startup invariant check. Tested via existing acceptance-test
  infrastructure under `tests/accepttest/`.

**Provable properties (test-level, not formal-proof-level — the feature
isn't load-bearing enough to warrant Kani):**

- ∀ email, `RegistrationPolicy::new(None).check_email(email) == Ok(())`.
- ∀ email, ∀ regex, `policy.check_email(email).is_ok() == regex.is_match(&normalize(email))`.
- The startup invariant is total: every (regex_set, verification_required)
  pair maps to either "start" or "fail" with no third state.

## Phase 2 — Test plan (to be written in Phase 2a, not now)

### Unit tests — `services/registration_policy.rs`

One test per row of the Edge Case Catalog above. Property test (proptest):
for any regex compiled from a small alphabet, `check_email` agrees with
`Regex::is_match(&email.trim().to_lowercase())`.

### Acceptance tests — `tests/accepttest/registration_domain.rs` (new)

1. With a `RegistrationPolicy` configured for `@understory\.io$`:
   - `register` with `kasper@understory.io` succeeds.
   - `register` with `attacker@evil.com` returns `permission_denied`,
     and no `users` row is left behind (cross-check repository).
   - `add_email` for an existing user with a non-matching email returns
     `permission_denied`.
   - `o_auth_login` (service-account auth) for a brand-new
     `evil@evil.com` returns `permission_denied`.
   - `o_auth_login` for an *existing* OAuth identity with a non-matching
     email succeeds (login, not signup).

### Startup tests — `tests/accepttest/startup.rs` or inline in `cli.rs`

1. Construct a `Config` with `domain_regex = Some(_)` and
   `require_email_verification = false` → the invariant function returns
   `Err`.
2. Same with `require_email_verification = true` → `Ok`.
3. Same with `domain_regex = None`, verification toggle in either state
   → `Ok`.

The invariant check should be a pure function (`fn validate_config(&Config) -> Result<()>`)
so it's unit-testable without spinning up the server.

## Phase 3 — Adversarial review checklist (filled in after impl)

Things the Adversary will probably hit and we should pre-empt:

- Does `add_email` actually get gated, or did we forget that path?
- Does the email get normalized identically in every call site, or are
  there normalization drift bugs between `register` and `add_email`?
- Is the regex applied to the *primary* email only, or to all emails?
  Spec says: applied to whichever email the operation is creating.
- Does the startup error message tell the operator *which* env var to set?
- Is the rejection error leaking the regex pattern into the client
  response? (Should not.)
- Are existing acceptance tests for `register` still green when the regex
  is *unset* (the default path)?
- Race: two concurrent `register` calls both pass the regex check and
  both attempt to insert the same email — DB unique constraint catches
  this; we should verify the error maps to a sane gRPC status (already
  the case via `error::to_status`, but worth checking).

## Resolved decisions

- **Scope = (B).** This task lands the flag (`FOREST_REQUIRE_EMAIL_VERIFICATION`)
  and the startup gate. The actual token-based email verification flow is
  a follow-up. See "Follow-up: real verification flow" below for what's
  already built and what's left.
- **`add_email` is gated by the same regex.** Same `RegistrationPolicy::check_email`
  call site, same `permission_denied` mapping. Otherwise a user signs up
  under `kasper@understory.io` and adds `attacker@anywhere.com` as a
  side-channel. (Phase 1a invariant 5.)
- **Regex anchoring: document, don't auto-wrap.** Operators write standard
  Rust `regex` syntax. We log the compiled pattern at startup so the
  operator can see what they actually configured. README/env-var doc
  explicitly notes that an unanchored `@understory\.io` matches
  `@understory.io.evil.com` and recommends terminating with `$`.

## Follow-up: real verification flow (out of scope here, but grounded)

Tracked separately. The pieces already exist on the forage side:

- **SMTP transport.** `apps/forage/crates/forage-server/src/email_consumer.rs` —
  lettre-backed `EmailConsumer` pulls jobs from NATS JetStream stream
  `FORAGE_EMAIL` and sends via SMTP. Configured via `SMTP_HOST`,
  `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM`, `SMTP_TLS`.
- **Wire format.** `forage_core::integrations::email::EmailEnvelope`
  (`to`, `subject`, `body_html`, `body_text`, `email_type`) on subject
  `forage.email.{email_type}`.
- **Token primitive.** `forage_core::auth::magic_link::{generate_magic_link_token,
  hash_magic_link_token, MagicLinkStore}` — SHA-256 hashed tokens, 15-min
  TTL, single-use, rate-limit-aware. Migration:
  `apps/forage/crates/forage-db/src/migrations/20260326000001_create_magic_link_tokens.sql`.
  Currently wired only for passwordless *login* at
  `/auth/magic-link` in `apps/forage/crates/forage-server/src/routes/auth.rs`.

The follow-up task is to extend the magic-link primitive with a second
`email_type` ("email_verification"), a forage route that consumes the
verification token and calls forest's existing `verify_email` RPC to
flip `user_emails.verified = true`. Forest itself stays mailer-free —
all SMTP and templating live in forage. The startup gate landing in
*this* task is the precondition that makes the follow-up actually
enforceable: today, `FOREST_REQUIRE_EMAIL_VERIFICATION` only blocks
misconfiguration; once the verification flow lands, we'll add per-request
checks that gate login (and possibly registration completion) on the
`verified` flag.
