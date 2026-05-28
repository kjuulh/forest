# DATA-252 — Auto-invite by verified email domain — manual smoke test

Quick browser-clicked verification of the full feature against a local
forest + forage stack. Run after `cargo test` is green to catch anything
the automated tests can't (rendering, CSS, real session cookies, real
JWT round-trips, the actual gRPC channel between forage and forest).

Expected duration: ~5 minutes.

## Setup (once per shell)

```bash
# Terminal 1 — forest control plane
cd apps/forest
mise run local:up           # postgres + minio
mise run db:migrate         # apply migrations incl. organisation_allowed_domains
cargo run -p forest-server  # listens on :4040

# Terminal 2 — forage web UI
cd apps/forage
mise run develop            # listens on :3000, points at local forest
```

You also need a domain you can publish a DNS TXT record under — either a
real domain you control, or `mise run local:dns-hosts <token>` if that
helper exists locally. Without one, you can't exercise the verification
step end-to-end; route + acceptance tests already cover the logic.

## Smoke

1. **Sign up admin.** Open <http://localhost:3000/auth/signup>. Register
   `admin-test@<your-domain>` (any password). If the local config
   requires email verification, click the magic link from the dev-mail
   log.
2. **Create org.** From the dashboard, create an org named `smoke-test`.
   Confirm you land at the org projects page.
3. **Open access settings.** Navigate to
   <http://localhost:3000/orgs/smoke-test/settings/access>.
   - Sidebar should highlight **Access**.
   - "Auto-invite by email domain" heading and form visible.
   - Empty-state message under the list.
4. **Add a domain — even a free-mail one.** Type `gmail.com` and submit.
   - It's accepted into the list with the tag *Awaiting verification*.
     The free-mail denylist is gone — DNS verification is the only
     security boundary.
   - Click **Remove** to take it back out.
5. **Add your real domain.** Type `<your-domain>` and submit.
   - The row appears with the *Awaiting verification* badge.
   - A code block shows the TXT record name (`_forest-verify.<your-domain>`),
     type (`TXT`), and the token to publish.
6. **Verify without DNS.** Click **Verify DNS** before publishing the
   record.
   - Page re-renders with an inline red flash: *"DNS TXT record at
     `_forest-verify.<your-domain>` not found yet…"*.
   - Row stays *Awaiting verification*.
7. **Publish the TXT record.** Add a TXT record at
   `_forest-verify.<your-domain>` with the token as the value. Wait for
   propagation (`dig TXT _forest-verify.<your-domain> +short` from your
   shell should show the token).
8. **Verify again.** Click **Verify DNS**.
   - Inline green flash: *"Verified ownership of `<your-domain>`."*.
   - Row badge flips to *DNS verified*. The TXT-instructions block and
     Verify button disappear.
   - Hitting Verify a second time is harmless — the success flash
     mentions "was already verified".
9. **Sign up a second user with a matching domain.** In a private/
   incognito window, register `joiner-test@<your-domain>`. Verify email
   if required.
10. **Banner appears on dashboard.** At <http://localhost:3000/> the
    joiner should see a blue banner: *"smoke-test allows anyone with a
    verified `@<your-domain>` address to join"* with a **Join** button.
11. **Accept.** Click **Join**.
    - Redirects to `/`.
    - Banner disappears (org list now includes `smoke-test`).
    - In the admin window, reload `/orgs/smoke-test/settings/members` —
      `joiner-test` is listed with role `member`.
12. **No double-banner.** Refresh joiner's dashboard. No banner for
    `smoke-test` anymore (already-member filter).
13. **Negative — unverified email.** Register a third user
    `unverified-test@<your-domain>` but **do not** verify the email
    (skip the magic link). Their dashboard must show no banner for
    `smoke-test`.
14. **Negative — DNS-unverified domain produces no banner.** As admin,
    add another domain (say `unrelated.example`) without ever verifying
    it. Sign up a fresh user `*@unrelated.example` and verify their
    email. Their dashboard must NOT show a join banner — the
    unverified-domain gate filters them out.

## What this catches that route tests don't

- Real DNS round-trips (resolver picks up `/etc/resolv.conf`, propagation
  delays are real, TXT chunking works).
- CSRF token round-trip through real cookies.
- Tailwind classes render visibly (green-info vs red-error flash; yellow
  *Awaiting* vs green *DNS verified* badges).
- Real gRPC channel between forage and forest accepts the new RPCs.

## What it does NOT cover (deferred / out of scope)

- Silent JIT for OAuth-verified emails (v1.1, `auto_join_oauth` policy —
  schema is ready, runtime rejects).
- Rate-limiting `AcceptJoinOffer` per user.
- A persistent audit log table — for now `tracing::info!` events at
  `org_allowed_domain.added` / `org_allowed_domain.removed` /
  `org_allowed_domain.verified` / `org_allowed_domain.verify_missing_txt` /
  `org_auto_invite.accepted` are the trail. If logs aren't shipped, the
  trail is non-durable.
