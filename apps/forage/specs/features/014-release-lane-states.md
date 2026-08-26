# Spec 014: Release Timeline Lane States

## Status: Phase 3 (Adversarial review)

## Problem

The release timeline's swim lane is the only thing on the overview that says
whether a release is finished. It got that wrong for the state that matters
most: a pipeline parked on a plan approval rendered as settled — visually
indistinguishable from one that had completed.

Reported from a live instance: *"when we trigger a release, and we're still in
the approval state, it right now is shown as the release is complete. It isn't,
it is pending still — like it would be if it was running a job. It is just
awaiting an action."*

### Root cause

`releaseEnvStates` resolved each environment from two sources: destination rows,
and **deploy** pipeline stages. An environment gated behind an unapproved plan
has neither a meaningful destination row (nothing has touched it, so it reports
`PENDING`) nor a started deploy stage. It therefore resolved to `pending` —
identical to a release merely queued behind an upstream stage.

The signal only exists on the **plan** stage, which nothing consulted.

Consequences, all silent:
- the dot rendered dashed-and-faded ("headed there, not started") rather than as
  something needing attention;
- `computeLaneBars` saw no in-flight environment, so no hatched segment was
  drawn, and the solid "deployed" bar from the release below ran straight past;
- nothing distinguished "waiting on a machine" from "waiting on a person",
  which are different in the one way that matters: only one of them resolves
  without somebody acting.

## Behavioral Contract

### Lane states

An environment on a release resolves to exactly one state:

| state | meaning |
|---|---|
| `awaiting` | a plan for this environment is parked, waiting for a person |
| `flight` | a deploy is QUEUED, ASSIGNED or RUNNING |
| `live` | this release is on that environment now |
| `pending` | headed there, nothing started, nothing blocking on a human |
| `past` | released there previously, or terminal (FAILED / TIMED_OUT / CANCELLED) |

Precedence is `awaiting > flight > live > stopped > pending > past`. `awaiting`
outranks `flight` deliberately: both mean "not finished", but only one is still
unfinished tomorrow unless somebody acts.

### Resolution rules
- Destination rows are authoritative where they exist.
- A plan stage that is awaiting approval marks **its** environment `awaiting`,
  regardless of what the destination row says. It is not gated on the
  environment being unseen — a `PENDING` destination row must not mask it.
- A plan stage is awaiting approval when `stage_type == "plan"` and
  `approval_status` is `AWAITINGAPPROVAL` **or** `AWAITING_APPROVAL`. Both
  spellings occur: the API emits Rust's Debug format.
- A deploy stage fills in an environment only when nothing else has claimed it.
- A rejected or cancelled plan is finished, not parked. It must not be
  `awaiting`.

### Rendering
- `awaiting` and `flight` are *unfinished*: the lane draws its hatched, animated
  in-flight segment and the dot pulses.
- `live`, `pending`, `past` are not: nothing animates.
- `awaiting` is visually distinct from `flight` — ringed rather than hollow, so
  it reads as blocked rather than working — and carries the title
  `Awaiting approval for <env>`.
- Every state in `DOT_PRIORITY` has an explicit template branch, except `past`,
  which is the intentional `{:else}` fallback.

### Non-Functional Requirements
- State resolution is pure and importable, so it can be tested without a browser
  (`src/lib/lane-states.js`).
- Missing or partial release objects must not throw: the timeline renders
  optimistically from SSE deltas, so `{}` reaches this code in practice.

## Verification

**Unit** — `src/lib/lane-states.test.js`, 32 tests over shared fixtures.
Confirmed non-vacuous: removing the plan-stage rule fails 8 of them.

**Rendered** — `gallery/capture.mjs` mounts the real component in Chromium once
per fixture and asserts what the DOM shows: the resolved state reached
`data-lane-states`, unfinished states animate and finished ones do not, and an
awaiting dot says so in words. Also confirmed non-vacuous — the regression
reproduces as `unfinished but nothing animates (hatched=0 pulsing=0)`, which is
the reported bug stated mechanically.

Fixtures are shared between the two (`src/lib/fixtures.js`) so a screenshot
cannot exist for a state no test covers, or vice versa.

## Known gaps
- The gallery needs the dev server running (`npm run gallery`) and is not wired
  into `mise run test`, which is `cargo test --workspace`. Running the frontend
  suites is currently a separate, manual step.
- `stopped` is in `DOT_PRIORITY` but never assigned by `releaseEnvStates` —
  terminal states resolve to `past`. Left as-is rather than removed: out of
  scope for this change, and it is referenced by priority comparisons.
