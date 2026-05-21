# TASKS/022 — `forest global add` runs `global sync` afterwards

**Status:** spec / pre-implementation
**Created:** 2026-05-21
**Driver:** user friction — after `forest global add <ref>`, the user expects the tool to be ready to use, but currently the shim dir may not be fully reconciled (catalogue subscriptions emit shims at add-time, but per-tool adds rely on `add_dependency` writing exactly one shim and don't re-sync the rest of `forest.cue`). The user-facing intent is "I added a tool; everything I declared should be in a usable state when this returns."

## Problem

`forest global add <org>/<name>[@<ver>]` and `forest global add <org>` write
to `forest.cue` and emit the shim(s) directly affected by that single command.
They do **not** invoke the same reconciliation path that `forest global sync`
runs. As a result:

1. Drift between `forest.cue` and the shim dir that existed before the `add`
   is not corrected by the `add`. The user has to remember to also run
   `forest global sync` to bring everything into a clean state.
2. The mental model "after `add`, my declared state is realised on disk" does
   not hold without the extra step.

We are NOT trying to pre-fetch binaries in this task. Binaries remain
lazily fetched on first `forest global run` / `which` (the existing
warm-path / cold-path logic in `service.rs:162`). The goal is purely shim
reconciliation parity with `forest global sync`.

## Goals

1. After a successful `forest global add …`, the shim directory is in the
   exact state `forest global sync` would have left it in for the current
   `forest.cue`.
2. The behaviour is opt-out via a `--no-sync` flag on `add`, for scripting /
   CI contexts that want to avoid the extra network calls `sync_shims` may
   make (`ListOrgTools` per subscribed org, `fetch_manifest` per per-tool
   dep without an explicit `shim_name`).
3. A sync failure after the dependency has already been persisted to
   `forest.cue` must not roll back the `add`: the dependency is recorded,
   the user is warned, and the exit code is still success. The user can
   re-run `forest global sync` later to converge.

## Non-goals

- Eager binary pre-fetching. (If we want that later, file a follow-up; the
  user explicitly opted for shim-only sync here.)
- Changing `sync_shims()` semantics. We invoke it as-is.
- Re-running sync after `remove`, `pin`, `unpin`, `ban`, `unban`. (Several
  of these already do partial reconciliation; out of scope here.)
- Adding sync to `subscribe_to_org` beyond what falls naturally out of
  re-running `sync_shims()` at the end. (Subscribe already emits shims for
  the catalogue it just wrote; `sync_shims()` will be a near no-op for the
  newly-written entries but will reconcile any pre-existing drift.)

## Behavioural Contract

### Surface

- New flag on `AddCommand`:
  - `--no-sync` (bool, default `false`). When set, suppresses the
    post-add reconciliation step. No alias.

### Postconditions

After `forest global add <ref>` returns successfully (exit 0):

- All preconditions of `AddCommand` continue to hold (dependency persisted,
  primary shim emitted as before).
- **Unless `--no-sync` is set:** `sync_shims()` has been invoked exactly
  once, AFTER the dependency / catalogue mutation has been persisted to
  `forest.cue`. The shim directory now matches the expected map computed
  from the post-add `forest.cue` (per the existing `sync_shims` algorithm
  in `service.rs:740`).
- The created/deleted shim lists from the sync step are surfaced to stderr
  in the same format as `forest global sync` uses today, prefixed so the
  user can tell it's the implicit sync (`sync (after add): N created, M
  deleted` followed by `  + <name>` / `  − <name>` lines, suppressed when
  both are empty).

### Failure modes

| Stage | Outcome | Why |
| --- | --- | --- |
| Dep resolution / config write fails | Exit non-zero, **no sync attempted**, no warning printed | Pre-existing behaviour. Nothing changed; nothing to reconcile. |
| Config write succeeds, `sync_shims()` returns `Err` | Exit 0, primary `add` output printed as today, then a single `warning: post-add sync failed: <err>; run \`forest global sync\` to retry` line on stderr | User's stated preference: dependency is recorded, warn but succeed. |
| `--no-sync` set | Exit 0, primary `add` output only. No sync line, no warning. | Explicit opt-out. |

## Interface Definition

```rust
// crates/forest/src/cli/global.rs
#[derive(Args)]
pub struct AddCommand {
    component: String,
    #[arg(long = "as")]
    as_shim: Option<String>,
    #[arg(long = "ban")]
    ban: Vec<String>,
    #[arg(long = "pin")]
    pin: Vec<String>,
    #[arg(long = "alias")]
    alias: Vec<String>,
    /// Skip the implicit `forest global sync` step after writing the
    /// dependency. Useful in scripts / CI that don't want extra network
    /// calls during `add`.
    #[arg(long = "no-sync", default_value_t = false)]
    no_sync: bool,
}
```

No changes to `GlobalService`. The CLI layer composes:

```text
AddCommand::execute:
  svc.add_dependency(..)  OR  svc.subscribe_to_org(..)   // persists state
  print primary outcome (existing eprintln!s)
  if !self.no_sync:
      match svc.sync_shims().await:
          Ok(out)  => print "sync (after add): …" lines if non-empty
          Err(e)   => eprintln!("warning: post-add sync failed: {e:#}; \
                                 run `forest global sync` to retry")
  Ok(())
```

## Edge Case Catalog

1. **`add <ref>` for a dep that is already in `forest.cue` at the same
   version** — `add_dependency` re-emits the shim; `sync_shims` finds the
   shim body matches and reports 0 created / 0 deleted; we suppress the
   "sync (after add): 0, 0" line to avoid noise.
2. **`add <ref> --as new-name` where the old shim name is still on disk**
   — `add_dependency` writes `new-name`, but doesn't delete the previous
   shim. `sync_shims` should delete the orphan IF and ONLY IF its
   `shim_name` was overridden in the dependency entry that we just
   updated. (Today, `add_dependency` on the same key overwrites the
   `shim_name` field in `forest.cue`, so the old shim becomes orphaned and
   `sync_shims` will delete it because it's no longer in `expected` and it
   carries `SHIM_MARKER`.) The sync output should show this deletion.
3. **Catalogue add (`add <org>`) where a previously-subscribed catalogue
   entry was banned via `--ban`** — `subscribe_to_org` won't emit a shim
   for banned entries, but a pre-existing shim from a prior subscription
   could remain. `sync_shims` deletes it (orphan + marker). Surface this
   in the sync output.
4. **`--no-sync` set on a per-tool add** — primary output unchanged, no
   sync line. Verify no `sync_shims` call is made (mock/spy or
   integration-level assertion that the shim dir is untouched beyond
   what `add_dependency` wrote).
5. **`sync_shims` fails because `ListOrgTools` is unreachable** — primary
   add succeeded against a per-tool dep that did not require the
   catalogue. We must NOT roll back. Single warning line. Exit 0.
6. **`add` fails before persisting** — we never reach the sync branch.
   No warning. Exit non-zero. (Verify nothing is printed about sync.)
7. **`sync_shims` deletes a shim the user manually created (no marker)**
   — pre-existing `sync_shims` invariant: only Forest-authored shims
   (with `SHIM_MARKER` on line 2) are deleted. The auto-sync inherits
   this. No new code needed; covered by existing `sync_shims` behaviour.
8. **Concurrent `add` and `sync`** — out of scope. The user-config layer
   already has no locking; this task does not introduce new contention.
9. **`--no-sync` plus `--as`** — flag combination is orthogonal. `--as`
   may leave an orphan from a prior shim name; with `--no-sync` the
   orphan remains until the next manual sync. Documented as expected.

## Non-Functional Requirements

- The implicit sync must add at most one extra round of work equivalent
  to one `forest global sync`. No nested loops, no repeated network calls.
- No new spawned threads/tasks. Use the existing async context.
- The warning path must not panic. `sync_shims` returns `anyhow::Result`;
  format with `{e:#}` to surface the full chain.
- Output to stderr only (matches existing `add` and `sync` conventions —
  see `cli/global.rs:82-91` and `:237-253`).

## Verification Architecture

### Provable Properties Catalog

| ID | Property | How verified |
| --- | --- | --- |
| P1 | If `--no-sync` is set, `sync_shims` is not called. | Unit / integration test asserting no shim-dir mutations beyond what `add_dependency`/`subscribe_to_org` produce. |
| P2 | If `--no-sync` is unset and add persists, `sync_shims` is called exactly once after persistence. | Integration test: pre-populate shim dir with an orphan Forest-authored shim, run `add` of an unrelated tool, observe the orphan is deleted. |
| P3 | A sync failure does not roll back the persisted dependency. | Integration test with a deliberately failing sync path (e.g. inject a missing/unreachable org for catalogue lookup during sync, while the add itself targets a per-tool dep that succeeded). Assert `forest.cue` retains the new dep and exit code is 0. |
| P4 | Existing `add` output is unchanged in content and order; sync output is appended. | Snapshot/golden test of stderr. |
| P5 | Exit code of `add` is unaffected by sync failure. | Process-level test. |

### Purity Boundary Map

- **Pure core (no change):** `GlobalService::add_dependency`,
  `subscribe_to_org`, `sync_shims` — all already encapsulate I/O behind
  the `GrpcClient`, `FsBackend`, and cache traits. This task does not
  cross the boundary.
- **Effectful shell (the change site):** `cli/global.rs::AddCommand::execute`.
  This is the only place where the new orchestration lives. The function
  remains a thin composition of service calls + eprintln output. No new
  business logic.

### Verification Tooling Selection

- Rust + existing `cargo test -p forest` for unit tests.
- The acceptance test in
  `crates/forest-server/tests/accepttest/global_tools_flow.rs` is the
  best place to add an end-to-end test that exercises the CLI through
  `add` → assert shim dir state → assert exit code. Mirror the style of
  existing flows there.
- No formal verification (Kani/Dafny) is justified for this change. The
  property set is behavioural / orchestration, not invariant-bearing
  pure-core logic.

### Property Specifications (test sketches)

```text
test: add_per_tool_runs_sync_by_default
  given: forest.cue with two declared deps A and B (shims present)
         manually remove B's shim from disk
  when:  `forest global add <A>` (re-add at same version, no --no-sync)
  then:  shim dir contains both A and B
         exit code 0
         stderr contains "sync (after add):" with B in the created list

test: add_per_tool_no_sync_skips_reconciliation
  given: forest.cue with two declared deps A and B
         manually remove B's shim from disk
  when:  `forest global add <A> --no-sync`
  then:  shim dir contains A but NOT B
         exit code 0
         stderr does NOT contain "sync (after add):"

test: add_warns_on_sync_failure
  given: a scenario where sync_shims will Err
         (e.g. catalogue subscription to an org whose ListOrgTools fails,
          combined with a per-tool add that succeeds independently)
  when:  `forest global add <per-tool-ref>`
  then:  forest.cue contains the new dep
         exit code 0
         stderr contains "warning: post-add sync failed:"
```

## Implementation Plan (post-spec)

1. Add `no_sync: bool` field to `AddCommand` (`cli/global.rs`).
2. After the `add_dependency` / `subscribe_to_org` branches, before
   `Ok(())`, insert the `if !self.no_sync { … }` block described above.
3. Suppress the sync line when both `created` and `deleted` are empty
   (avoid noisy "0, 0" output for the common case).
4. Add unit tests in `cli/global.rs` if a testable seam exists, plus the
   acceptance test in `accepttest/global_tools_flow.rs`.
5. Update `cli/docs.rs` only if it auto-generates from clap (verify; if
   it does, the new flag is picked up automatically).

## Open questions

None at this point — clarified with the architect:

- Sync scope: shim reconciliation only, no eager binary fetch.
- Failure mode: warn but succeed.
- Opt-out: `--no-sync` flag, default off.
