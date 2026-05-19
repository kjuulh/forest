# Adversarial Review: Forage Client (Post Phase 2)

## Date: 2026-03-07
## Scope: Full project review - architecture, workflow, business, security

---

## 1. Forage Needs a Clear Ownership Boundary

Forage is entirely dependent on forest-server for its core functionality. Every
route either renders static marketing content or proxies to forest-server.

What does forage own today?
- Auth? No, forest-server owns it.
- User data? No, forest-server owns it.
- Component registry? Future, and forest-server will own that too.
- Deployment logic? Future, likely another backend service.

This isn't wrong - forage is the web product layer on top of forest's API layer.
But this intent isn't crystallized anywhere. The PITCH.md lists a huge roadmap
(deployments, managed services, billing) without clarifying what lives in forage
vs. what lives in forest-server or new backend services.

**Why this matters**: Every architectural decision (session management, crate
boundaries, database usage) depends on what forage will own. Without this
boundary, we risk either building too much (duplicating forest-server) or too
little (being a dumb proxy forever).

**Action**: Write a clear architecture doc or update PITCH.md with an explicit
"forage owns X, forest-server owns Y, future services own Z" section. At
minimum, forage will own: web sessions, billing/subscription state, org-level
configuration, and the web UI itself. Forest-server owns: users, auth tokens,
components, deployments.

Comment:
Forage in the future is going to have many services that forest is going to be relying on, hence the brand, and site. Also forest-client might be fine as a UI for forest itself, but a flutter app isn't that great at web apps, and we need something native for SEO and the likes.
We could adopt the forest-client as the dashboard tbd. Forage is the business entity of forest.

---

## 2. The Crate Structure Is Premature

Five crates today:
- **forage-db**: 3 lines. Re-exports `PgPool`. No queries, no migrations.
- **forage-core**: ~110 lines. A trait, types, 3 validation functions.
- **forage-grpc**: Generated code wrapper. Justified.
- **forage-server**: The actual application. All real logic lives here.
- **ci**: Separate build tool. Justified.

The "pure core / effectful shell" split sounds principled, but `forage-core`
is mostly type definitions. The `ForestAuth` trait is defined in core but
implemented in server. The "pure" validation is 3 functions totaling ~50 lines.

**forage-db is dead weight.** There are no database queries, no migrations, no
schema. It exists because CLAUDE.md says there should be a db crate. Either
remove it or explicitly acknowledge it as a placeholder for future forage-owned
state (sessions, billing, org config).

**Action**: Either consolidate to 3 crates (server, grpc, ci) until there's
a real consumer for the core/db split, or commit to what forage-core and
forage-db will contain (tied to decision #1). Premature crate boundaries add
compile time and cognitive overhead without benefit.

Comment
Lets keep the split for now, we're gonna fill it out shortly

---

## 3. Token Refresh Is Specified But Not Implemented

The spec says:
> If access_token expired but refresh_token valid: auto-refresh, set new cookies

Reality: `RequireAuth` checks if a cookie exists. It doesn't validate the
token, check expiry, or attempt refresh. When the access_token expires,
`get_user()` fails and the user gets redirected to login - losing their
session even though the refresh_token is valid.

Depending on forest-server's token lifetime configuration (could be 15 min to
1 hour), users will get randomly logged out. This is the single most impactful
missing feature.

**Action**: Implement BFF sessions (spec 003) which solves this by moving
tokens server-side and handling refresh transparently.

---

## 4. The get_user Double-Call Pattern

Every authenticated page does:
1. `get_user(access_token)` which internally calls `token_info` then `get_user`
   (2 gRPC calls in `forest_client.rs:161-192`)
2. Then page-specific calls (e.g., `list_tokens` - another gRPC call)

That's 3 gRPC round-trips per page load. For server-rendered pages where
latency = perceived performance, this matters.

The `get_user` implementation calls `token_info` to get the `user_id`, then
`get_user` with that ID. This should be a single call.

**Action**: Short-term, BFF sessions with user caching (spec 003) eliminates
repeated get_user calls. Long-term, consider pushing for a "get current user"
endpoint in forest-server that combines token_info + get_user.

We should be able to store most of this in the session, with a jwt etc. That should be fine for now

---

## 5. Cookie Security Gap

`auth_cookies()` sets `HttpOnly` and `SameSite=Lax` but does NOT set `Secure`.

The spec explicitly requires:
> forage_access cookie: access_token, HttpOnly, **Secure**, SameSite=Lax

Without `Secure`, cookies are sent over plain HTTP. Access tokens can be
intercepted on any non-HTTPS connection.

**Action**: Fix immediately regardless of whether BFF sessions are implemented.
If BFF sessions come first, ensure the session cookie sets `Secure`.

---

## 6. The Mock Is Too Friendly

`MockForestClient` always succeeds (except one login check). Tests prove:
- Templates render without errors
- Redirects go to the right places
- Cookies get set

Tests do NOT prove:
- Error handling for real error scenarios (only one bad-credentials test)
- What happens when `get_user` fails mid-flow (token expired between pages)
- What happens when `create_token` or `delete_token` fails
- What happens when forest-server returns unexpected/partial data
- Behavior under concurrent requests

**Action**: Make the mock configurable per-test. A builder pattern or
`Arc<Mutex<MockBehavior>>` would let tests control which calls succeed/fail.
Add error-path tests for every route, not just login.

---

## 7. Navigation Links to Nowhere

`base.html.jinja` links to: `/docs`, `/components`, `/about`, `/blog`,
`/privacy`, `/terms`, `/docs/deployments`, `/docs/registry`, `/docs/services`.

None exist. They all 404.

This isn't a code quality issue - it's a user experience issue for anyone
visiting the site. Every page has a nav and footer full of dead links.

**Action**: Either remove links to unbuilt pages, add placeholder pages with
"coming soon" content, or use a `disabled` / `cursor-not-allowed` style that
makes it clear they're not yet available.

Comment
add a place holder and a todo, also remove the docs, we don't need that yet. also remove the blog and other stuff. Lets just stick with the main things. components and the login etc.

---

## 8. VSDD Methodology vs. Reality

VSDD.md describes 6 phases: spec crystallization, test-first implementation,
adversarial refinement, feedback integration, formal hardening (fuzz testing,
mutation testing, static analysis), and convergence.

In practice:
- Phase 1 (specs): Done well
- Phase 2 (TDD-ish): Tests written, but not strictly red-green-refactor
- Phase 3 (adversarial): This review
- Phases 4-6: Not done

The full pipeline includes fuzz testing, mutation testing, and property-based
tests. None of these exist. The convergence criterion ("adversary must
hallucinate flaws") is unrealistic - real code always has real improvements.

This isn't a problem if VSDD is treated as aspirational guidance rather than
a strict process. But if the methodology doc says one thing and practice does
another, the doc loses authority.

**Action**: Either trim VSDD.md to match what's actually practiced (spec ->
test -> implement -> review -> iterate), or commit to running the full pipeline
on at least one feature to validate whether the overhead is worth it.

Comment: Write in claude.md that we need to follow the process religiously

---

## 9. The Pricing Page Sells Vapor

The pricing page lists managed deployments, container runtimes, PostgreSQL
provisioning, private registries. None of this exists. The roadmap has 4
phases before any of it works.

The landing page has a "Get started for free" CTA leading to `/signup`, which
creates an account on forest-server. After signup, the dashboard is empty -
there's nothing to do. No components to browse, no deployments to create.

If this site goes live as-is, you're either:
- Collecting signups for a waitlist (fine, but say so explicitly)
- Implying a product exists that doesn't (bad)

**Action**: Add "early access" / "waitlist" framing. The dashboard should
explain what's coming and what the user can do today (manage tokens, explore
the registry when it exists). The pricing page should indicate which features
are available vs. planned.

Comment: Only add container deployments for now, add the other things as tbd, forget postgresql for now

---

## 10. Tailwind CSS Not Wired Up

Templates use Tailwind classes (`bg-white`, `text-gray-900`, `max-w-6xl`, etc.)
throughout, but the CSS is loaded from `/static/css/style.css`. If this file
doesn't contain compiled Tailwind output, none of the styling works and the
site is unstyled HTML.

`mise.toml` has `tailwind:build` and `tailwind:watch` tasks, but it's unclear
if these have been run or if the output is committed.

**Action**: Verify the Tailwind pipeline works end-to-end. Either commit the
compiled CSS or ensure CI builds it. An unstyled site is worse than no site.

---

## 11. forage-server Isn't Horizontally Scalable

With in-memory session state (post BFF sessions), raw token cookies (today),
and no shared state layer, forage-server is a single-instance application.
That's fine for now, but it constrains deployment options.

This isn't urgent - single-instance Rust serving SSR pages can handle
significant traffic. But it should be a conscious decision, not an accident.

**Action**: Document this constraint. When horizontal scaling becomes needed,
the session store trait makes it straightforward to swap to Redis/Postgres.

Comment: Set up postgresql like we do in forest and so forth

---

## Summary: Prioritized Actions

### Must Do (before any deployment)
1. **Fix cookie Secure flag** - real security gap
2. **Implement BFF sessions** (spec 003) - fixes token refresh, caching, security
3. **Remove dead nav links** or add placeholders - broken UX

### Should Do (before public launch)
4. **Add "early access" framing** to pricing/dashboard - honesty about product state
5. **Verify Tailwind pipeline** - unstyled site is unusable
6. **Improve test mock** - configurable per-test, error path coverage

### Do When Relevant
7. **Define ownership boundary** (forage vs. forest-server) - shapes all future work
8. **Simplify crate structure** or justify it with concrete plans
9. **Align VSDD doc with practice** - keep methodology honest
10. **Plan for horizontal scaling** - document the constraint, prepare the escape hatch

---

## What's Good

To be fair, the project has strong foundations:

- **Architecture is sound.** Thin frontend proxying to forest-server is the
  right call. Trait-based abstraction for testability is clean.
- **Spec-first approach works.** Specs are clear, implementation matches them,
  tests verify the contract.
- **Tech choices are appropriate.** Axum + MiniJinja for SSR is fast, simple,
  and right-sized. No over-engineering with SPAs or heavy frameworks.
- **Cookie-based auth proxy is correct** for this kind of frontend (once moved
  to BFF sessions).
- **CI mirrors forest's patterns** - good for consistency across the ecosystem.
- **ForestAuth trait** makes testing painless and the gRPC boundary clean.
- **The gRPC client** is well-structured with proper error mapping.

The issues are about what's missing, not what's wrong with what exists.
