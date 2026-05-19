# TASKS/021 — SDK Bootstrap & CUE Onboarding

**Status:** spec / pre-implementation
**Created:** 2026-05-19
**Driver:** end-to-end onboarding friction surfaced during the live publish + lazy fetch verification of TASKS/018 (global tools).

## Problem

A fresh user with a fresh `forest-server` cannot run `forest publish` against
their server, even on the happy path. The chain is:

1. `forest publish` (and `forest components publish`) parses the project's
   `forest.cue` + `forest.component.cue` via `cue export`.
2. Those CUE files import `forest.sh/forest/sdk@v0`.
3. `cue` resolves the import against `$CUE_REGISTRY` (mise sets this to
   `localhost:4042+insecure` — i.e. the forest-server's built-in OCI
   read-only registry at `/v2/...`).
4. The OCI registry only serves modules that were committed via the
   server's `commit_upload` path (see `oci_registry::publish_cue_module`).
5. Nobody has committed `forest.sh/forest/sdk@v0` yet, so the import 404s.

Result: `forest publish` fails with
`cannot find package "forest.sh/forest/sdk@v0": module not found` — and there
is no `forest admin bootstrap-sdk`, no `forest-server --seed-sdk`, and no
documented manual workaround other than writing a one-shot Rust helper
(which is how cycles 9–14 of TASKS/018 did their live verification, and
how the verification on 2026-05-19 did it again).

A secondary issue, also blocking until fixed: the per-component
`cue.mod/module.cue` files shipped in `examples/global-tools/forest-*` were
written for an older CUE module schema (`language: version: "v0.10.0"`,
no `source:`, no `deps:`). Modern `cue` requires `source: {kind: "self"}`
and explicit `deps:` to resolve any non-builtin import — without them
`cue export` fails before the SDK lookup even happens. (Fixed in this PR
across all five example projects.)

## Goals

1. A first-run user can publish their first component without writing
   ad-hoc tooling. Specifically: `forest-server serve` followed by
   `forest auth register`, `forest organisation create`, then
   `forest publish` from one of the `examples/global-tools/forest-*`
   directories must succeed.
2. Onboarding is observable: when SDK bootstrap is required and missing,
   `forest publish` should produce a clear, actionable error pointing at
   the bootstrap step — not a CUE-resolver internal error.
3. The bootstrap is idempotent and survives server restarts. Re-running
   it must be a no-op (sha-matched layer + manifest in S3).

## Non-goals

- Auto-installing the `cue` binary (deferred FU1 from TASKS/018).
- Replacing the OCI-registry-backed CUE module distribution with something
  else.
- Distributing SDK versions older than the current `v0.x` line.

## Proposed approach (sketch, not locked)

Two complementary mechanisms:

### A. Server-side auto-seed at boot

`forest-server serve` reads the in-tree `cue/forest-sdk/` directory at
startup and publishes it via `oci_registry::publish_cue_module` if no
manifest exists at `forest.sh/forest/sdk/manifests/v<X>.<Y>.<Z>` yet.

- Implementation surface: a new `seed_sdk_module` function called
  during server startup, after migrations, before serving traffic.
- The SDK version is read from `cue/forest-sdk/cue.mod/module.cue`
  (already declares `module: "forest.sh/forest/sdk@v0"`; we'd add a
  semantic version tag the auto-seeder treats as the target version).
- Skipped if `FOREST_DISABLE_SDK_AUTOSEED=1` so production deployments
  that manage SDK versions out-of-band can opt out.

### B. CLI command for explicit bootstrap

`forest admin publish-sdk [--path <dir>] [--version <v>]` — same code path
as (A), but invoked manually. Useful for:

- Air-gapped servers where the in-tree SDK isn't present.
- Publishing custom SDK forks.
- CI environments that prefer explicit setup.

Authz: requires the service-account API key (`FOREST_SERVICE_ACCOUNT_API_KEY`)
since this is a cross-org infra operation, not a per-user action.

### C. Error message improvement (independent of A/B)

When `forest publish` encounters a CUE resolver error matching the SDK
import pattern, replace the raw stderr passthrough with:

```
error: failed to resolve forest.sh/forest/sdk@v0

The CUE SDK module hasn't been published to your CUE_REGISTRY
(currently $CUE_REGISTRY). For a freshly-bootstrapped forest-server,
run:

  forest admin publish-sdk            (or restart with auto-seed enabled)

See TASKS/021-sdk-bootstrap.md for background.
```

This is a small, isolated win and worth landing even before A/B.

## Edge cases to think through

- **E1**: Two `forest-server` instances race to auto-seed on first boot.
  `publish_cue_module` is idempotent (content-addressed layer blob; the
  manifest write is last-writer-wins on identical content), so both
  succeed.
- **E2**: SDK in tree is newer than what's already in the registry. Seeder
  must compare versions and publish only the missing ones — never overwrite.
- **E3**: Operator wants the registry to lag behind the in-tree SDK
  (e.g., to test backwards-compat). Honored via `FOREST_DISABLE_SDK_AUTOSEED=1`
  + explicit `forest admin publish-sdk --version vX.Y.Z`.
- **E4**: User runs `forest publish` against a registry that does NOT host
  the SDK (e.g., a forest-server behind a mismatched CUE_REGISTRY env).
  Error message from (C) tells them what to do.

## Acceptance

- Live: `git clean -xfd && cargo run -p forest-server -- serve` →
  `forest auth register` → `forest organisation create` →
  `cd examples/global-tools/forest-hello && forest publish` succeeds
  without any out-of-band setup.
- Test: a forest-server integration test that boots the server with
  auto-seed enabled, queries `GET /v2/forest.sh/forest/sdk/manifests/v0.2.0`,
  asserts 200 + correct content.
- Test: `forest admin publish-sdk` round-trips against a server that
  starts with `FOREST_DISABLE_SDK_AUTOSEED=1`.
- The four `examples/global-tools/forest-*` projects publish cleanly
  end-to-end (TOOL_BINARY + HYBRID_COMPONENT + 3 × TOOL_EXTERNAL).

## Out of this spec

- A registry-mirror feature (e.g., proxy `cuelang.org/...` reads to a
  local cache) — orthogonal.
- Cross-server SDK pinning / lockfile semantics — orthogonal.
- SDK versioning policy / deprecation — handled in TASKS/018 follow-ups.

## Related findings (already fixed in the same PR as this spec)

- Server: password-requirements failures now map to gRPC `InvalidArgument`
  with the rule list, instead of a generic `internal error`
  (see `forest-server/src/grpc/error.rs`, `native_credentials.rs`).
  `forest auth register --help` now documents the password rules.
- CLI: `forest auth token create/list` default `--user-id` to the
  currently logged-in user, removing the awkward "look up your own UUID
  before you can mint a token" step.
- CLI: `forest eval zsh|bash` now emits
  `${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shims` instead of a
  literal `$HOME/.cache/...`, so the eval output honors XDG and matches
  the runtime path resolution in `global::paths`.
- Examples: every `examples/global-tools/forest-*/cue.mod/module.cue`
  declares `source: {kind: "self"}` + `deps:` for the SDK. They will
  publish cleanly once item A or B above is in place.
