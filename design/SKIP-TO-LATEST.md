# Skip to latest — collapsing a queue of pending releases

*DATA-817. Status: design + prototype. Policy ships default OFF. Not rolled out.*

On a high-traffic project the deploy queue for one destination grows faster than it
drains. Twelve merges land in twenty minutes, forest queues twelve releases behind the
one it is running, and then deploys all twelve in order — eleven of which are already
contained in the twelfth. Every one of those eleven costs a full terraform apply or ECS
rollout, and the twelfth — the one anybody actually wants — lands last.

This document defines a policy that collapses that queue: when the policy is on, forest
deploys the newest pending release for a target and marks the older pending ones
`SUPERSEDED`.

The whole thing rests on one claim, argued in §1: **forest already has exactly one
queue, and both release pipelines and individual deployments queue in it.** If that
holds, there is one mechanism, one decision point, and one new status — not two of each.

---

## 0. What forest does today

The relevant tables (`crates/forest-server/migrations/20250622092446_initial.sql`):

```
release_intents      one per `forest release ...`. `stages` JSONB is the pipeline DAG,
                     NULL for a non-pipeline release. status: ACTIVE | SUCCEEDED |
                     FAILED | CANCELLED.

release_states       one row per (intent, destination) — the unit of work a runner
                     executes. status: QUEUED | ASSIGNED | RUNNING | SUCCEEDED |
                     FAILED | CANCELLED | TIMED_OUT.

release_events       append-only log; `emit_event` writes an event and the new
                     materialised status in one transaction.
```

Two indexes carry the queue semantics:

```sql
CREATE UNIQUE INDEX idx_release_active
    ON release_states (project_id, destination_id)
    WHERE status IN ('ASSIGNED', 'RUNNING');

CREATE INDEX idx_release_queue_position
    ON release_states (project_id, destination_id, queued_at ASC)
    WHERE status = 'QUEUED';
```

At most one release in flight per `(project_id, destination_id)`; everything else waits,
oldest first. `pick_queued_releases` enforces the first half (`NOT EXISTS (… ASSIGNED |
RUNNING …)`) and `next_queued_for_destination` the second.

Two components drive work:

- **`scheduler.rs`** — subscribes to `forest.release.queued`, sweeps every 5s, and calls
  `SchedulerInner::handle_release(release_id)`. That function is the *only* place in
  forest where a `QUEUED` release becomes `ASSIGNED`.
- **`intent_coordinator.rs`** — the pipeline saga. `evaluate(intent_id)` walks the DAG
  and, when a Deploy or Plan stage becomes ready, **inserts `release_states` rows with
  `status = 'QUEUED'`** and publishes `forest.release.queued`.

There is one existing, manual version of this feature: `forest release create --force`
cancels every `QUEUED` release for the destination before enqueuing
(`release_event_store.rs::create_release`, the `params.force` branch). It marks them
`CANCELLED` with `error_message = 'superseded by force release'` — a failure-shaped
status and an error message for something that is neither. This design is the automatic,
opt-in, correctly-typed version of that.

---

## 1. The unifying concept

> **There is one queue. A pipeline run does not have its own.**

A pipeline run is `ACTIVE` from the moment it is created — it never sits "pending" as a
run. What queues is its Deploy/Plan stage's `release_states` rows, inserted by
`intent_coordinator::evaluate` into the same table, with the same `status = 'QUEUED'`,
against the same `(project_id, destination_id)` key, drained by the same scheduler as a
non-pipeline release. The only difference between the two surfaces is that a pipeline
release carries a non-NULL `stage_id`.

So the answer to "is it one mechanism or two" is: **one**, and it is not a compromise —
it is where forest's queue actually lives. Superseding at the intent level would have
required inventing a second queue that does not exist.

### Target key

```
target_key = (project_id, destination_id)
```

Two pending releases are *in the same queue* iff they share this key. This is forest's
own key — the unique index, the sweep's `NOT EXISTS`, `next_queued_for_destination`, and
`--force` all use it.

**Not `(project, environment)`.** A destination belongs to exactly one environment, so
environment is implied, but the reverse is not true: an environment can hold several
destinations, and they are independent queues that drain in parallel. Collapsing across
an environment would strand a pending release at destination B because a newer one
arrived at destination A. The *policy* is configured per environment (matching
soak-time, branch-restriction and external-approval, which all key on
`target_environment`); the *collapse* happens per destination inside it.

### "Pending"

```
pending = release_states.status = 'QUEUED'
```

One definition, both surfaces. `ASSIGNED` and `RUNNING` are not pending — which is
where §3's "never interrupt a running deploy" comes from structurally rather than as a
check.

### Ordering — what "latest" means

Within a target key, releases are totally ordered by

```
(queued_at ASC, release_id ASC)
```

and "latest" is the maximum. Both components are monotone in wall-clock: `queued_at`
defaults to `now()`, and `release_id` is `Uuid::now_v7()`, which is time-ordered. The
`release_id` tiebreak makes the order total and deterministic when two rows share a
timestamp. This is the order forest's queue already drains in
(`idx_release_queue_position`), so the policy is not introducing an ordering — it is
reading the existing one.

**Deliberately not commit order.** forest records `Ref { commit_sha, branch, … }` on an
annotation but has no commit graph: nothing in the schema relates two SHAs, so forest
cannot ask "is B a descendant of A" without an oracle it does not have. Enqueue order is
the only total order forest owns. §5 covers what a real descendant guard would need, and
what we can enforce instead.

---

## 2. What gets skipped, and to what state

When the policy is on for a target key, the decision is:

> Of the `QUEUED` releases for this target key, the maximum proceeds. Every strictly
> older one transitions to `SUPERSEDED`.

### The new status

A new terminal, non-failure release status:

```
SUPERSEDED  — this release never ran, because a newer release for the same target
              was already pending when its turn came.
```

reached by a new event `release.superseded`, whose only valid predecessor is `QUEUED`:

```rust
Self::Superseded => &["QUEUED"],    // valid_from_statuses()
```

with the reason recorded in `release_states.error_message` and in the event payload:

```
superseded by release <uuid> (supersede-pending policy 'collapse-prod')
```

`ReleaseStatus::Superseded` is `is_finalized() == true`, `is_failure() == false`,
`is_success() == false`. Nothing that counts failures counts it; nothing that counts
deploys counts it.

### The word "superseded", and the one collision to watch

"Superseded" is the honest name for what happens — a newer release took this one's place
— and it is the word `--force` already uses for the same act
(`'superseded by force release'`). The policy is `supersede_pending`, the status is
`SUPERSEDED`, and the reason text names the release that did the superseding.

One collision to be aware of rather than to design around: forage uses "superseded" in
its swimlanes for a related but different thing — a release that **did deploy** and is no
longer live (DATA-660 / DATA-661:
`superseded_release_stays_a_release_not_a_hidden_commit`, *"the superseded release
deployed here, but is no longer live"*). That one is a **display concept derived from
deploy history**, not a stored status; this one is a stored status on a release that
never deployed. They can coexist — the swimlane already distinguishes "deployed here" from
"did not deploy" — but forage's renderer has to tell them apart, and it does not today.
That is step 2 of the rollout (§8), and it is the one piece of UI work this change
requires.

### Pipeline stages

A skipped release must not turn its pipeline run red. Deriving a Deploy stage's status
from its children today is two-way — all `SUCCEEDED` → `Succeeded`, otherwise `Failed` —
so without a change, collapsing a queue would page whoever owns the project. The
derivation becomes three-way:

| children | stage |
|---|---|
| all `SUCCEEDED` | `Succeeded` |
| any `FAILED` / `CANCELLED` / `TIMED_OUT` | `Failed` (unchanged) |
| otherwise, any `SUPERSEDED` | `Superseded` *(new)* |

`StageStatus::Superseded` is terminal and counts toward `is_pipeline_complete`. A `PENDING`
stage downstream of a `Superseded` one becomes `Superseded` too, with
`"upstream stage superseded"` — not `Cancelled`, which in this codebase means somebody
cancelled it.

Intent status gains the matching value:

| stages | intent |
|---|---|
| all `Succeeded` | `SUCCEEDED` |
| any `Failed` | `FAILED` |
| otherwise any `Superseded` | `SUPERSEDED` *(new)* |
| otherwise | `CANCELLED` |

Failure dominates skip, on purpose: a run that half-deployed and then failed is a
failure, whatever happened to its remaining stages.

### The latest always proceeds

The decision names **only strictly older** releases. The maximum of the pending set is
never an argument to `release.superseded`. §4 turns that into an invariant with a proof.

---

## 3. In-progress work — recommendation: let it finish

A deploy already `ASSIGNED` or `RUNNING` when a newer release arrives.

**Recommendation: let it finish.** Implemented; the only mode in the prototype.

This is not a check we perform — it is structural. The decision point runs inside
`handle_release`, which returns early for anything that is not `QUEUED`, and the
collapse reads only `QUEUED` rows. An in-flight release is invisible to the mechanism.
That is worth more than a flag: the failure mode "the policy killed a running terraform
apply" cannot be reached by a bug in the collapse logic, only by writing a new code path.

Why it is the right default, beyond safety-by-construction:

- **forest's destinations are not transactional.** `terraform apply`, a Flux
  reconciliation, an ECS rolling update — none can be cancelled into a defined state. A
  cancel halfway through an apply leaves real infrastructure in a state no plan
  describes, and forest would have no record of what actually landed.
- **The cost is bounded and is already today's behaviour.** Letting it finish means one
  extra deploy of one intermediate artifact. The queue behind it still collapses. On the
  12-merges example, the policy takes 12 deploys down to 2 (the one already running plus
  the newest), not to 1 — which is the entire win.
- **Cancelling buys minutes; the queue collapse buys the rest.** Nearly all the waste is
  in the queued tail, not the head.

`cancel_in_progress` exists in the config so the contract is settled, and **validation
rejects `true`** with a pointer to this section. Turning it on needs, at minimum:

1. a runner-side cancellation protocol (`WorkAssignment` is one-way today);
2. a per-destination-type "is this safe to abandon mid-flight" capability —
   `forest/noop@1` trivially is, `forest/terraform@1` is not;
3. a defined state for a destination whose apply was interrupted, and a way to report it
   that is neither `SUCCEEDED` nor `FAILED`.

That is its own piece of work, and it should not ride along with this one.

### Pipeline runs parked ahead of the queue

The case the release-level mechanism does not shortcut: run A is sitting in a `Wait` or
`Gate` stage, and runs B and C blow past it in dev. Nothing collapses at the wait stage —
A holds no queue position.

**This is a latency and waste gap, not a correctness gap**, and the reason is worth
stating precisely. A, B and C all eventually reach the prod destination. A gets there
first and deploys (it is in flight; §3 lets it finish). B and C queue behind it, and when
A completes, the collapse fires at prod: C proceeds, B is `SUPERSEDED`. The final state of
prod is C, the newest. An older release never wins. A deployed an intermediate artifact
to prod, which is exactly the "let in-progress finish" trade already accepted above.

A run-level supersede — cancel an older `ACTIVE` run that is parked in a `Wait` or `Gate`
stage when a newer run for the same project reaches the same stage — is a sound follow-up
(abandoning a `Wait` or a `Gate` has no side effect in flight, unlike a `Deploy`). It is
deliberately **out of scope for this prototype**: it is a second decision point, in a
second component, with its own races, and it buys latency rather than correctness.

---

## 4. Correctness

Let `Q(k)` be the set of `release_states` rows with `status = 'QUEUED'` and target key
`k`, ordered by `(queued_at, release_id)`.

### Liveness — the latest pending release always deploys

> **L.** For any target key `k`, `max(Q(k))` is never transitioned to `SUPERSEDED`, and is
> eventually dispatched.

*Never skipped.* The collapse function reads `Q(k)`, computes `latest = max`, and emits
`release.superseded` only for members **strictly less than** `latest`. So a skip of some
release `r` implies some reader observed an `r' > r` that was `QUEUED` at that moment. If
`r` were the global maximum of the committed set, no such `r'` exists in any reader's
view — a reader's view is a subset of committed rows. Therefore the maximum is never
named as a skip target, by any reader, at any time. A reader working from a *stale* view
is still safe: a stale view contains fewer rows, so its `latest` is ≤ the true latest,
and the rows it skips are all strictly below its own `latest` — all strictly below the
true one.

*Eventually dispatched.* The scheduler is woken by `forest.release.queued` and, failing
that, by the 5s sweep; `handle_release` on the latest finds nothing newer and proceeds.
When the scheduler is holding a release that turns out **not** to be the latest, it
republishes `forest.release.queued` for the latest before returning, so the collapse does
not depend on the sweep for progress.

### Safety — an older release never wins over a newer one

> **S.** For a target key `k`, no release dispatches after a strictly newer release from
> `k` has dispatched.

Unchanged from today, and that is the point. The queue is FIFO by `(queued_at,
release_id)` and at most one release per key is in flight (`idx_release_active`). The
policy only ever *removes* elements from `Q(k)`; it never reorders, never re-queues, and
never dispatches out of order. Removing elements from a FIFO cannot produce an inversion.

### Race safety and idempotence

> **R.** Concurrent evaluation of the collapse, by any number of schedulers, cannot
> strand the latest, cannot double-deploy, and cannot leave a target key empty of work.

The single primitive is `emit_event`, which already does

```sql
SELECT status … FROM release_states WHERE release_id = $1 FOR UPDATE
```

and refuses a transition whose predecessor is not in `valid_from_statuses()`. That gives
us, per row, a compare-and-swap. Every interesting interleaving reduces to it:

| interleaving | outcome |
|---|---|
| Two schedulers skip the same older release | One wins; the loser's `emit_event` fails the valid-from guard and is treated as a no-op. Idempotent. |
| Scheduler A dispatches `r` (`QUEUED`→`ASSIGNED`) while B tries to skip `r` (B saw a newer `r'`) | Row-level CAS on `r` picks one. If A wins, `r` deploys and `r'` queues behind it — safe, one extra deploy. If B wins, A's `Assigned` fails and A returns early (the existing `"already transitioned"` path). Either way `r'` stays `QUEUED` and is the maximum. |
| A newer `r'` commits *after* a scheduler read `Q(k)` | The scheduler skips a subset of rows all older than its own `latest`; `r'` is untouched and is picked up on the next wake. |
| Two schedulers both decide to proceed with the same latest | `emit_event(Assigned)` CAS lets exactly one through; the other returns early. This is today's behaviour, unchanged. |
| A scheduler's candidate was retired (or claimed) between it reading the status and the collapse reading the queue | The candidate is simply absent from the pending set. The decision returns `LeftQueue` and the scheduler stops. |

That last row is the one the race test found rather than the one the design predicted.
The first version treated "candidate not in the pending set" as the default case and said
*proceed* — which would have handed the scheduler a release it must not dispatch.
`emit_event`'s valid-from guard would have refused the transition and nothing would have
deployed twice, so the invariants held; but relying on a downstream guard to catch a
decision this function got wrong is not the same as getting it right, and it is the kind
of thing that is correct until someone adds a second caller. Hence three verdicts —
`Proceed`, `Superseded`, `LeftQueue` — rather than a boolean.

The collapse is **one function, called from one place** — `handle_release`, immediately
after the `status != "QUEUED"` early return and before any dispatch work. Not scattered
across `create_release`, the coordinator, and the gRPC layer.

### What is deliberately *not* claimed

The policy does not guarantee that only one release deploys. It guarantees that the
newest pending one does, and that no older *pending* one does. A release already in
flight finishes, by design (§3).

---

## 5. Configuration

New policy type alongside the existing three, in the same `oneof` and the same org-rule
selector — nothing bespoke:

```proto
enum PolicyType {
    …
    POLICY_TYPE_SUPERSEDE_PENDING = 4;
}

message SupersedePendingConfig {
    // Environment this applies to. Destinations inside it collapse
    // independently — see §1.
    string target_environment = 1;

    // Only supersede a pending release when the newer one is on the same
    // branch. The nearest guard forest can actually enforce against a
    // divergent release; see below.
    bool same_branch_only = 2;

    // Cancel an in-flight deploy and jump to the newest. NOT IMPLEMENTED —
    // validation rejects `true`. See §3.
    bool cancel_in_progress = 3;
}
```

Wired through `Policy`, `CreatePolicyRequest`, `UpdatePolicyRequest` and `OrgPolicyRule`
at field 13, so it selects per project/env through the existing org-rule machinery.

**Default OFF.** There is no default-on path: a project with no `supersede_pending` policy
behaves exactly as it does today, and the collapse function returns "proceed" without
touching a row. A policy that skips deploys must never turn itself on.

### The descendant guard

The ask was a guard so we do not skip a *genuinely divergent* release — a hotfix from a
release branch that is not an ancestor of the newer one. The real form of that check is
"is the skipped release's commit an ancestor of the newer one", and forest cannot answer
it: annotations carry a bare `commit_sha` with no parent relation, and forest never
clones the repo.

What forest *can* enforce, from data it already has on the annotation, is
`same_branch_only`: supersede only when both releases name the same
`Ref.branch`. Two releases on `main` are almost always ancestor-related; a hotfix from
`hotfix/x` against a `main` release is not, and will not be collapsed. It is a sound
approximation in the direction of caution — it can decline to skip something that was
skippable, never the reverse.

A true descendant guard needs a git oracle (a `GET /compare` against the forge, or
forest recording `parent_sha` at annotate time). **Open question for the rollout** —
recorded here, not built.

### Observability — a skipped deploy is never silent

Per skipped release:

- `release_events` gains a `release.superseded` row carrying the reason. This is the
  append-only audit log; it is already what `forest release show --logs-only` and the
  event-store views read.
- `org_events` gains a `status_changed` row via `emit_event`'s existing outbox — the
  feed forage's activity view renders.
- `forest.release.status.<intent_id>` is published, so `forest release show --follow`
  and `WaitRelease` report the terminal state instead of hanging.
- `release_states.error_message` names the release that superseded it.
- `tracing::info!` with the target key and the collapsed count.

Deliberately **no per-release notification**: collapsing thirty pending releases would
mean thirty Slack pings for deploys that did not happen. Whether the *winner's*
notification should say "superseded 11 older pending releases" is an open question for
the rollout — it needs a count threaded to the notification, and it is the right place
for the summary if we want one.

---

## 6. Interplay with the other policies

Precedence, stated once: **supersede-pending is a selection policy, not a gate.** It never
blocks a release; it chooses which pending release is the candidate. Every gating policy
then runs against that candidate exactly as it does today. Selection happens first,
gating second — so a gate can never be evaluated against a release that is about to be
skipped, and a skip can never bypass a gate.

Concretely, `handle_release` becomes:

```
status != QUEUED            → return                     (unchanged)
supersede-pending           → supersede self and return, or continue as latest  (new)
soak_time                   → defer                      (unchanged)
… dispatch
```

`evaluate_for_environment` reports `supersede_pending` as **always passed**, with a
descriptive reason, so `forest project policy evaluate` shows the policy exists without
corrupting `all_passed`.

**Soak time.** A soaking release is `QUEUED` — the scheduler defers it, it does not leave
the queue. So it *is* eligible to be superseded, and it should be: the whole point of
soak time is that prod waits for dev to bake, and if three newer releases have piled up
behind the soaker, the newest is the one that should deploy when the soak clears. The
newest then soaks on its own terms — soak time is evaluated against the candidate after
selection, so the winner never inherits the loser's soak credit.

**External approval.** The newest pending release needs *its own* approval; approvals are
recorded per `(policy, release_intent_id)` and are never transferred. Superseding an
unapproved pending release is therefore safe and correct: it was not going to deploy
without approval, and neither will its replacement. The one behaviour worth naming: if
someone has approved release #7 and #9 arrives, #7 is skipped and #9 needs a fresh
approval. That is right — the approval was for #7's contents. Worth putting in the
release notes, because it will surprise someone.

**Branch restriction.** Enforced at the gRPC layer, before a release ever enters the
queue, so a restricted release is never pending and never a supersede candidate. No
interaction. `same_branch_only` is a separate mechanism with a separate purpose — do not
conflate them.

**Gates (`GateStageConfig`).** A gate parks a pipeline *run*, not a queue position (§3's
last subsection). Supersede-pending does not touch it in this design.

**Release pipelines generally.** Composition is automatic, because the collapse happens
below the pipeline: the coordinator queues releases, the scheduler collapses the queue,
and the coordinator observes `SUPERSEDED` children and marks the run `SUPERSEDED`. The
coordinator needs no knowledge of the policy.

---

## 7. Prototype scope

Built:

- `POLICY_TYPE_SUPERSEDE_PENDING` + `SupersedePendingConfig`, in `policies.proto` and
  `org_rules.proto`, additive; server domain type, validation, CRUD, org-rule
  materialisation, CLI.
- `ReleaseStatus::Superseded` + the `release.superseded` event, valid only from `QUEUED`.
- `services/supersede.rs` — the decision, pure and separately tested, plus the one
  DB-facing collapse.
- The call site: `SchedulerInner::handle_release`, one place.
- `StageStatus::Superseded` + three-way stage derivation + downstream propagation +
  `SUPERSEDED` intent status in `intent_coordinator`.
- Every other place that enumerates terminal statuses. `WaitRelease` is the one that
  mattered: its stage-terminal check listed `SUCCEEDED | FAILED | CANCELLED`, so a
  collapsed pipeline run would have left `forest release create` waiting forever on a
  release that was never going to run. Intent finalisation and the CLI's status icons
  are the same shape of omission, found the same way — by grepping for the places that
  spell the status set out rather than asking `is_finalized()`.

  The `current_release` view is the one place `SUPERSEDED` is deliberately *not* added:
  it answers "what is live at this destination", and a release that never deployed is
  not an answer to that.

Not built, and named as such: cancel-in-progress (§3), run-level supersede of a parked
pipeline run (§3), a true descendant guard (§5), a summary notification (§5).

## 8. What a real rollout needs

1. Land the proto + status + mechanism with **no policy configured anywhere**. Every
   test forest has asserts today still passes; behaviour is bit-identical.
2. Confirm forage renders `SUPERSEDED` as a neutral terminal state, not as a failure and not
   as a hidden commit — this is the DATA-660 lesson, and the naming question in §2 should
   be settled before, not after.
3. Enable on one high-traffic non-critical project in dev. Watch `release.superseded` counts
   against merge rate.
4. Decide `same_branch_only` as a default. It is off in the config; it may deserve to be
   on.
5. Only then prod, per project, opt-in.

forest deploys itself, so step 1's deploy is Kasper's call and is staged by hand.
