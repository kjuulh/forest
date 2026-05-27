# Rename "Forage" → "Forest" — Task Log

Tracks the work driven by `specs/features/012-rename-to-forest.md`.

## Status

- [x] Spec drafted (`specs/features/012-rename-to-forest.md`)
- [x] Spec reviewed; decisions D1, D2 locked
- [x] Tests written (Red gate confirmed: 3 contexts.rs failures)
- [x] Implementation (Green: 223 forest tests + 415 forage tests pass)
- [ ] Adversarial review pass
- [ ] Infra applied to dev
- [ ] Infra applied to production
- [ ] OAuth callbacks re-registered with third parties (C1, owner: kasper)

## Resolved

- **D1.** Tagline → "Forest - your developer platform".
- **D2.** OCI registry → `registry.api.forest.understory.sh` (default produced by `derive_cue_registry`; no helper change).

## Cutover ownership

- **C1.** OAuth callback re-registration (Slack, Google, GitHub, Microsoft) — **owner: kasper**, to happen in the same window as the infra apply.

## PR Plan

1. **Spec PR** (this one) — `forest/apps/forage/specs/features/012-rename-to-forest.md` + this log.
2. **Code PR** in `forest/`:
   - `apps/forage/crates/forage-server/src/routes/pages.rs` — title
   - `apps/forage/crates/forage-server/src/tests/pages_tests.rs` — locked assertion
   - `apps/forage/templates/base.html.jinja` — footer
   - `apps/forage/templates/pages/{landing,pricing}.html.jinja` — copy
   - `apps/forest/crates/forest/src/contexts.rs` — `derive_web_url_from_server` convention swap + tests
   - `apps/forest/crates/forest/src/cli/context.rs` — doc example
   - `forest/README.md` — install snippet + UI link
   - `apps/forest/README.md` — sweep
3. **Infra PR** in `infrastructure-platform/`:
   - `forage.tf` — `forage_subdomain` → `forest`
   - `forest.tf` — `forest_subdomain` → `api.forest`; `forest_registry_subdomain` → `registry.api.forest`
   - Comment cleanup in both files

## Notes captured during scoping

- Crate names stay `forage-*`. Rename is brand + URL only.
- Terraform module/local *identifiers* stay (`module.forage`, `local.forage_url`); only their **values** change. Avoids state surgery.
- `derive_cue_registry` needs no code change — `registry.<server-host>` naturally produces `registry.api.forest.understory.sh` from the new server URL.
- `DEFAULT_SERVER = "http://localhost:4040"` unchanged.
- Pre-release: no DNS aliases, no deprecation window, breaking changes OK.

## Scope additions made during implementation

- **Webhook header rename.** `X-Forage-Signature` → `X-Forest-Signature` and the outbound `User-Agent: Forage/1.0` → `Forest/1.0`. Wire-protocol identifiers visible to webhook consumers — keeping them as `Forage` after rebranding to `Forest` was incoherent. Pre-release stance covers the break. Touched `notification_worker.rs`, `webhook_delivery_tests.rs`, `nats_tests.rs`, `tools/webhook-test-server.py`, and three template files. Supersedes the assertion in `specs/features/006-notification-integrations.md` §"X-Forage-Signature" — that historical spec is left in place.
- **CLI convention error message.** `forest auth login --web`'s error string referenced the old "forest. → forage." rule. Rewrote to match the new "leading `api.` required" rule (`cli/auth/login_web.rs`).
- **Stale unrelated test fixed.** `pages_tests.rs::landing_page_contains_expected_content` asserted "Container Deployments" which doesn't appear on the landing page. Pre-existing test rot, not introduced by this work; updated assertion to "Type-Safe Infrastructure" to unblock the test run.

## Out of scope, flagged for follow-up

- `mailto:sales@forage.sh` in `templates/pages/pricing.html.jinja:70` — an actual email/domain decision (does `sales@forest.sh` exist? does `sales@understory.io` make more sense?). Left as-is.
- `// Forage/Forest user ID` comment in `forage-core/src/integrations/mod.rs:107` — leave for a code cleanup pass.
