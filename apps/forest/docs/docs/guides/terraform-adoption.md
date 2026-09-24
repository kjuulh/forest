# Adopting existing infrastructure into a terraform component

How to move resources that another Terraform state already owns (a Terraform
Cloud workspace, say) under a `forest/terraform@1` component, without
destroying and recreating them, and without anyone running `terraform import`
or `terraform state rm` by hand.

The short version: **do it in configuration.** Both halves of a move are
expressible as blocks that go through a normal, reviewed plan:

| side | block | available from |
|---|---|---|
| the new owner (the forest component) | `import { to = ..., id = ... }` | Terraform/OpenTofu 1.5; `for_each` on import from OpenTofu 1.7 |
| the old owner (the old workspace) | `removed { from = ...; lifecycle { destroy = false } }` | Terraform 1.7 / OpenTofu 1.7 |

forest-server runs a pinned OpenTofu (see `apps/forest/Dockerfile`), which has
both.

## Why config rather than the CLI

forest's state lives behind its own HTTP backend and is only reachable from
inside a release (forest#282). The CLI route (`import`, then `state rm` in the
old workspace) needs state access on both sides, and leaves no review trail.
The config route needs neither: an `import` block shows up in the plan as
`will be imported`, the plan stage's approval *is* the review, and the block
is idempotent once the resource is in state.

## The convention for components

A component that can adopt what it would otherwise create should:

1. **Use the old owner's names.** If the names are the same, every import ID
   can be derived from the spec, and adopting becomes a switch rather than a
   list of IDs the consumer has to find.
2. **Offer one spec field, `adopt: bool`,** that renders an `import` block for
   every resource the spec would create, with IDs derived from those names.
   Off by default.
3. **Say in its README which resources hold data** (queues with messages in
   flight, schedule groups with live schedules, log groups), because those are
   the ones a mistaken plan must never replace. `prevent_destroy` on them turns
   that mistake into a failed plan.

## The sequence, for one service

1. **Old state lets go first.** A PR to the old workspace adding a `removed`
   block with `destroy = false` for each of the service's addresses. Its plan
   must say *will no longer be managed* for each one and **nothing else**.
   Apply it.
2. **forest adopts.** A release with `adopt: true`. The plan must show one
   `will be imported` per resource, plus only the differences you intend (tags,
   policy changes the component makes on purpose). Anything that says
   *destroy* or *replace*: reject.
3. **Clean up.** Drop `adopt` in a follow-up. Both plans are empty.

### The dual-ownership window

Between the two applies, exactly one of two things is true:

- **Old side first (recommended):** for a while *nobody* manages the
  resources. They keep running untouched. The risk is only that a change made
  in that window isn't tracked anywhere.
- **forest first:** for a while *both* states manage them, and an apply of the
  old workspace can undo, or destroy, what forest just adopted. That's the
  failure this whole procedure is avoiding.

So do the old side first, and keep the window short: land step 2 right after
step 1.

## Traps

- **Idempotent creates.** SQS `CreateQueue` and SNS `Subscribe` return the
  existing resource instead of failing on a name clash. A component that
  *creates* (no `import` block) with a name someone else owns will silently
  share it, and delete it on destroy. Adopt deliberately, or pick distinct
  names.
- **`moved` blocks don't cross states.** They rename addresses *within* one
  state, which is what a component upgrade that renames resources needs. They
  can't move something from the old workspace to forest; that's what
  `import` + `removed` are for.
- **Approve, then apply promptly.** Until forest#284 lands, the apply stage
  re-plans rather than applying the plan that was approved, so anything that
  changes in between is applied unreviewed. Once it lands, a plan that went
  stale in the meantime (because the old workspace applied, say) fails the
  apply instead. Either way: re-run the release and review the new plan.
