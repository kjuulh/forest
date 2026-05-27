# 012 — Rename "Forage" UI to "Forest"; move API to `api.forest`

## Intent

Today the platform has two user-facing pieces with confusing names:

| Piece | Today | After |
| --- | --- | --- |
| Managed web UI (axum app in `apps/forage/`) | "Forage" at `forage.understory.sh` | "Forest" at `forest.understory.sh` |
| gRPC API (CLI server in `apps/forest/`) | `forest.understory.sh` | `api.forest.understory.sh` |
| OCI registry (HTTP port 4042 on the API task) | `registry.forest.understory.sh` | `registry.api.forest.understory.sh` |

The Rust crate names (`forage-server`, `forage-core`, `forage-db`) stay as
they are — this spec covers only user-visible names and URLs. Renaming
crates is a follow-up if and when desired.

We are pre-release. Breaking changes are acceptable. No DNS aliases, no
deprecation window, no dual-publish.

## Scope

Three repos / trees are touched:

1. `forest/apps/forage/` — UI brand strings and tests
2. `forest/apps/forest/` — CLI defaults, web-URL convention helper, README
3. `infrastructure-platform/` — Terraform locals, certs, ALB host rules,
   Cloudflare DNS, and the root `README.md` install snippet

## Behavioral Contract

### B1. UI title

The forage axum server's landing page MUST render `<title>Forest - your
developer platform</title>`. The footer MUST read `© 2026 Forest`.

Touch:
- `apps/forage/crates/forage-server/src/routes/pages.rs:27`
- `apps/forage/templates/base.html.jinja:180`
- `apps/forage/crates/forage-server/src/tests/pages_tests.rs:26` (test
  string locked to the new title)
- Marketing copy in `apps/forage/templates/pages/landing.html.jinja` and
  `pricing.html.jinja` — replace user-visible "Forage" with "Forest"
  where it refers to the product. References to *Forest the IaC tool*
  stay unchanged.

### B2. Infra domain swap

In `infrastructure-platform/`:

```hcl
# forage.tf
forage_subdomain = var.environment == "production" ? "forest" : "forest.${var.environment}"
forage_domain    = "${local.forage_subdomain}.understory.sh"

# forest.tf
forest_subdomain          = var.environment == "production" ? "api.forest" : "api.forest.${var.environment}"
forest_domain             = "${local.forest_subdomain}.understory.sh"
forest_registry_subdomain = var.environment == "production" ? "registry.api.forest" : "registry.api.forest.${var.environment}"
forest_registry_domain    = "${local.forest_registry_subdomain}.understory.sh"
```

Everything that keys off these locals (ACM certs, Cloudflare records,
ALB host-header listener rules, env vars `EXTERNAL_HOST`,
`FOREST_WEB_APP_URL`, `FORAGE_HOST`) updates by cascade.

Comments mentioning the old hostnames (`forest.tf:3-4`, `forage.tf:3,
9, 159, 202`, `forest.tf:192, 237, 249-252`) are corrected in the same
PR.

**Cert/DNS replacement is expected.** Terraform will recreate the ACM
certs for both modules. We accept the validation blip.

### B3. CLI defaults & conventions

In `apps/forest/crates/forest/src/contexts.rs`:

- `derive_web_url_from_server` (line ~124) — the current convention is
  "first label `forest` → `forage`". This MUST become "strip leading
  `api.` label, force https". Examples after the change:
  - `https://api.forest.understory.sh` → `https://forest.understory.sh`
  - `https://api.forest.dev.understory.sh` → `https://forest.dev.understory.sh`
  - `http://localhost:4040` → `http://localhost:3000` (unchanged)
  - Server URLs without an `api.` prefix → `None` (caller surfaces a
    "configure web_url" error, as today)
- Doc comment at lines 27–35 updated to reflect the new convention
  (server `https://api.forest.understory.sh` → registry
  `registry.api.forest.understory.sh`).
- `derive_cue_registry` keeps its `registry.<server-host>` formula —
  no code change. With server `api.forest.understory.sh` it yields
  `registry.api.forest.understory.sh`, which matches B2.
- Tests at lines 719–769 updated to the new hostnames.

In `apps/forest/crates/forest/src/cli/context.rs:280`, the doc example
`https://forest.understory.sh` becomes `https://api.forest.understory.sh`.

`DEFAULT_SERVER = "http://localhost:4040"` is unchanged. There is no new
production default — the operator still supplies the server via
`FOREST_PROFILE` / `forest context provision`.

**Fallback chain unchanged.** `ContextEntry::resolve_web_url` still
walks: (1) explicit `web_url` field, (2) `FOREST_WEB_URL` env,
(3) `derive_web_url_from_server` convention, (4) `None`. Only the
convention itself swaps. Concretely: a context provisioned via
`install.sh` with `FOREST_PROFILE=name=...,server=https://api.forest.understory.sh`
and no `web=` segment MUST resolve to `https://forest.understory.sh`
for `forest auth login --web`, with no `--web-url` flag required.

### B4. READMEs & install

- `forest/README.md:12` — install snippet's `FOREST_PROFILE` server
  value becomes `https://api.forest.understory.sh`.
- `forest/README.md:45` — `apps/forage/` description and link become
  `forest.understory.sh` (the UI), with text noting the crate is still
  named `forage` internally.
- `forest/scripts/install.sh` — no code change required. The script
  has no hardcoded URLs; it just passes `--server $profile_server`
  (and optionally `--web-url $profile_web`) to `forest context
  provision`. Interpretation of the server URL — including deriving
  the web URL when `web=` is omitted from `FOREST_PROFILE` — lives in
  the CLI per B3. Sweep comments for stale `forest. → forage.`
  references and update.
- `apps/forest/README.md` and any `docs/` references — sweep for
  `forest.understory.sh` and update.

### B5. Out of scope

- Renaming Rust crates (`forage-*` → something else)
- Renaming Terraform module names (`module.forage`, `module.forest`)
  — keeping these stable avoids state moves and large resource
  recreates beyond the certs.
- Renaming the `forage-blobs` / `forest-blobs` S3 buckets, Aurora
  cluster identifiers, or Secrets Manager secret names.
- Renaming GitHub container image refs (`ghcr.io/understory-io/forage`,
  `ghcr.io/understory-io/forest`).
- Renaming `forage_url`, `forage_domain` etc. Terraform locals — those
  stay as identifiers; only their *values* change.

## Edge Cases

- **E1.** A forest CLI context provisioned before the rename will have
  `server = "https://forest.understory.sh"` cached on disk. After the
  rename that points at the UI, not the gRPC API, and will fail.
  Acceptable per pre-release stance; users re-run `forest context
  provision` (or the install snippet). Document in the PR description.
- **E2.** `derive_web_url_from_server` falls back to `None` for any
  server URL that doesn't match the convention. With the new "strip
  `api.`" rule, an old context with `forest.understory.sh` will
  return `None` and surface the "configure web_url" error — clean
  failure, not silent misdirection.
- **E3.** ALB host-header collision during `terraform apply`: the new
  forage rule wants `forest.understory.sh` while the old forest rule
  still claims it. Apply order matters: forest module's rule must drop
  the old host header *before* the forage module's rule adds it.
  Single `terraform apply` should resolve this via dependency graph
  because both rules ultimately depend on the same listener — but
  verify in the plan output. If apply fails, run forest module first
  (`-target`), then forage.
- **E4.** Any third-party OAuth/webhook callback URLs registered with
  Slack, Google, etc. that point at `forage.understory.sh` MUST be
  re-registered with the new host. Enumerate during implementation
  (grep `apps/forage/crates/forage-server` for callback URL builders).

## Non-Functional Requirements

- No behaviour change beyond hostnames and brand strings.
- Cert blip during cutover acceptable (<5 min DNS-01 validation per
  cert). Schedule outside business hours if possible.
- No data migration. No DB changes. No secret rotation.

## Verification Strategy

This is a mechanical rename. The pure-core/effectful-shell distinction
doesn't meaningfully apply — the work is config and string changes.
Formal verification is overkill.

**Provable properties:** none worth formal proof.

**Test coverage:**
- `pages_tests.rs` assertion on the new `<title>` (string-locked).
- New unit tests in `contexts.rs` covering `derive_web_url_from_server`:
  - `https://api.forest.understory.sh` → `Some("https://forest.understory.sh")`
  - `https://api.forest.dev.understory.sh` → `Some("https://forest.dev.understory.sh")`
  - `https://forest.understory.sh` (no `api.` prefix) → `None`
  - localhost case unchanged.
- Existing CLI tests at `contexts.rs:719-769` updated to new hostnames.
- `terraform plan` reviewed in PR; expected diff = cert replacement +
  Cloudflare record change + ALB listener rule host-header swap.

## Resolved Decisions

- **D1.** Tagline: **"Forest - your developer platform"**.
- **D2.** OCI registry stays at the host `derive_cue_registry` produces
  by default → `registry.api.forest.understory.sh`. No special-case in
  the helper.

## Cutover (not code)

- **C1.** Re-registration of OAuth callback URLs (Slack, Google,
  Microsoft, GitHub) with the new `forest.understory.sh` host. Owner:
  **kasper**. Must complete in the same window as the infra apply or
  logins break. Enumerate during infra PR review.

## Implementation Order

1. Land this spec (this PR). Resolve Q1–Q3.
2. Code PR in `forest/`: B1, B3, B4 — UI strings, CLI convention,
   READMEs. Mergeable independently; no infra dependency.
3. Infra PR in `infrastructure-platform/`: B2. Coordinate apply with a
   one-shot OAuth callback re-registration window per Q3.

Per VSDD: tests first inside each code PR. Red → Green → Refactor.
