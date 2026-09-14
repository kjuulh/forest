//! Collapse a queue of pending releases to the newest one.
//!
//! On a high-traffic project the deploy queue for a destination grows faster
//! than it drains, and forest deploys every queued release in turn even though
//! all but the last are already contained in the last. With the
//! `supersede_pending` policy on, forest deploys the newest pending release for
//! a target and marks the ones it overtook `SUPERSEDED`.
//!
//! The full design, including the correctness argument this module implements,
//! is in `design/SKIP-TO-LATEST.md`. The three things worth knowing here:
//!
//! **One queue, both surfaces.** A pipeline run has no queue of its own — its
//! Deploy and Plan stages insert `release_states` rows with `status = 'QUEUED'`
//! against the same `(project_id, destination_id)` key that a non-pipeline
//! release uses, and the same scheduler drains them. So one mechanism covers
//! pipeline runs and individual deployments; there is no second path.
//!
//! **One decision point.** [`collapse_queue`] is called from exactly one place,
//! `SchedulerInner::handle_release`, which is the only place in forest where a
//! `QUEUED` release becomes `ASSIGNED`.
//!
//! **The latest is never superseded.** [`plan`] only ever names releases that
//! are strictly older than some other pending release it read, so the maximum of
//! the queue can never be an argument to `release.superseded` — for any reader,
//! at any time, from any view. That plus `emit_event`'s row-level
//! compare-and-swap (`SELECT … FOR UPDATE` + a valid-from guard admitting only
//! `QUEUED`) is what makes this race-safe rather than merely careful.

use std::collections::HashMap;

use anyhow::Context;
use uuid::Uuid;

use super::{
    policy::PolicyRegistry,
    release_event_store::{EventPayload, ReleaseEventStore, ReleaseEventType},
};

/// One pending release in a target queue, as the decision sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub release_id: Uuid,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    /// The branch the release was annotated from, when forest recorded one.
    /// Only consulted under `same_branch_only`.
    pub branch: Option<String>,
}

/// One release retired in favour of a newer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supersession {
    pub superseded: Uuid,
    pub by: Uuid,
}

/// What should happen to the release the scheduler is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It is the newest of its group: dispatch it.
    Proceed,
    /// A newer pending release for the same target took its place.
    Superseded,
    /// It is not pending any more — another scheduler retired it or claimed it
    /// while this one was deciding. Distinct from `Superseded` because an
    /// operator reading the logs wants to know which of the two happened.
    LeftQueue,
}

/// What the scheduler should do with the release it is holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Older pending releases to retire, each with the release that overtook it.
    /// Never contains the newest release of its group.
    pub supersessions: Vec<Supersession>,
    /// Whether the release the scheduler is holding should be dispatched.
    pub verdict: Verdict,
    /// When the candidate is not the one to dispatch: the release that is, so
    /// the scheduler can nudge it rather than wait for the next sweep.
    pub nudge: Option<Uuid>,
}

/// Decide which of `queue` to retire, given that the scheduler is holding
/// `candidate`.
///
/// Pure, so the invariants can be tested without a database.
///
/// The queue is totally ordered by `(queued_at, release_id)` — the order
/// forest's own `idx_release_queue_position` drains in, with `release_id`
/// (a `Uuid::now_v7`, and so time-ordered) breaking ties deterministically.
/// Each group collapses to its own maximum.
///
/// Without `same_branch_only` there is one group: the whole queue collapses to
/// its newest. With it, each branch is its own sub-queue and collapses to its
/// own newest — which still leaves the overall newest surviving and last,
/// because it is the maximum of its own branch too. A release forest recorded
/// no branch for is its own group, so an unattributable release is never
/// retired on a guess.
pub fn plan(candidate: Uuid, queue: &[Pending], same_branch_only: bool) -> Plan {
    let mut ordered: Vec<&Pending> = queue.iter().collect();
    ordered.sort_by(|a, b| {
        a.queued_at
            .cmp(&b.queued_at)
            .then_with(|| a.release_id.cmp(&b.release_id))
    });

    // The newest release of each group.
    let mut newest_per_group: HashMap<GroupKey<'_>, Uuid> = HashMap::new();
    for p in &ordered {
        let key = group_key(p, same_branch_only);
        // `ordered` ascends, so the last write per key is that group's maximum.
        newest_per_group.insert(key, p.release_id);
    }

    let mut supersessions = Vec::new();
    // A candidate that is not in the pending set has already left QUEUED —
    // superseded or claimed by another scheduler between the caller reading its
    // status and us reading the queue. Defaulting that to "proceed" would hand
    // the scheduler a release it must not dispatch; `emit_event`'s valid-from
    // guard would refuse the transition, but relying on a downstream guard to
    // catch a decision this function got wrong is not the same as getting it
    // right.
    let mut verdict = if queue.iter().any(|p| p.release_id == candidate) {
        Verdict::Proceed
    } else {
        Verdict::LeftQueue
    };
    let mut nudge = None;

    for p in &ordered {
        let key = group_key(p, same_branch_only);
        let winner = newest_per_group[&key];
        if winner == p.release_id {
            continue; // the newest of its group — never superseded
        }
        supersessions.push(Supersession {
            superseded: p.release_id,
            by: winner,
        });
        if p.release_id == candidate {
            verdict = Verdict::Superseded;
            nudge = Some(winner);
        }
    }

    // Nothing overtook the candidate, but it is gone: point the scheduler at
    // whatever is still pending for the target, so the queue keeps draining.
    if verdict == Verdict::LeftQueue {
        nudge = ordered.last().map(|p| p.release_id);
    }

    Plan {
        supersessions,
        verdict,
        nudge,
    }
}

/// Which sub-queue a pending release collapses within.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GroupKey<'a> {
    /// The guard is off: one queue, everything collapses to its newest.
    All,
    Branch(&'a str),
    /// forest recorded no branch for this release, and the guard is on. Keyed
    /// by the release's own id so it is a group of one — it is the newest of
    /// its group, and so survives. Retiring it would mean guessing that a newer
    /// release contains it, which is the one thing the guard exists to refuse.
    Unattributed(Uuid),
}

fn group_key(p: &Pending, same_branch_only: bool) -> GroupKey<'_> {
    if !same_branch_only {
        return GroupKey::All;
    }
    match p.branch.as_deref() {
        Some(b) => GroupKey::Branch(b),
        None => GroupKey::Unattributed(p.release_id),
    }
}

/// The single decision point. Called by the scheduler for a `QUEUED` release,
/// before any dispatch work.
///
/// Returns [`Verdict::Proceed`] unchanged for every project that has not opted
/// in — a project with no `supersede_pending` policy pays one indexed lookup and
/// behaves exactly as it does today.
#[allow(clippy::too_many_arguments)]
pub async fn collapse_queue(
    event_store: &ReleaseEventStore,
    policy_registry: &PolicyRegistry,
    nats: &async_nats::Client,
    candidate: Uuid,
    project_id: Uuid,
    destination_id: Uuid,
    environment: &str,
) -> anyhow::Result<Verdict> {
    let Some((policy_name, config)) = policy_registry
        .supersede_pending_for_environment(&project_id, environment)
        .await
        .context("look up supersede_pending policy")?
    else {
        return Ok(Verdict::Proceed);
    };

    let queue = pending_for_target(event_store, project_id, destination_id).await?;
    let plan = plan(candidate, &queue, config.same_branch_only);

    if plan.supersessions.is_empty() && plan.verdict == Verdict::Proceed {
        return Ok(Verdict::Proceed);
    }

    let mut retired = 0usize;
    for s in &plan.supersessions {
        let reason = format!(
            "superseded by release {} (supersede-pending policy '{}')",
            s.by, policy_name,
        );

        // A failure here is the expected outcome of a lost race, not an error:
        // another scheduler either retired this release first or dispatched it
        // while we were deciding. Either way the row has left QUEUED, the
        // valid-from guard refuses the transition, and there is nothing to do.
        match event_store
            .emit_event(
                s.superseded,
                ReleaseEventType::Superseded,
                EventPayload {
                    reason: Some(reason),
                    ..Default::default()
                },
                None,
            )
            .await
        {
            Ok(()) => {
                retired += 1;
                notify_superseded(event_store, nats, s.superseded).await;
            }
            Err(e) => {
                tracing::debug!(
                    release_id = %s.superseded,
                    "supersede: release already left the queue, leaving it alone: {e}"
                );
            }
        }
    }

    tracing::info!(
        %project_id,
        %destination_id,
        environment,
        policy = %policy_name,
        retired,
        pending = queue.len(),
        verdict = ?plan.verdict,
        "supersede: collapsed pending queue to the newest release"
    );

    if plan.verdict == Verdict::Proceed {
        return Ok(Verdict::Proceed);
    }

    // The candidate is not the newest. Hand the scheduler's attention to the
    // release that is, rather than waiting up to 5s for the next sweep.
    if let Some(winner) = plan.nudge {
        let _ = nats
            .publish("forest.release.queued", winner.to_string().into())
            .await;
    }

    Ok(plan.verdict)
}

/// Tell the things watching a release that it reached a terminal state.
///
/// A superseded deploy must never be silent: `emit_event` has already written
/// the `release.superseded` event and the `org_events` row inside its
/// transaction, so what is left is waking the live watchers — `WaitRelease` /
/// `forest release show --follow`, which would otherwise hang on a release that
/// is never going to run, and the pipeline coordinator, which has to re-derive
/// the stage this release belonged to.
async fn notify_superseded(event_store: &ReleaseEventStore, nats: &async_nats::Client, id: Uuid) {
    let Ok(state) = event_store.get_release_state(&id).await else {
        return;
    };

    let subject = format!("forest.release.status.{}", state.release_intent_id);
    let payload = serde_json::json!({
        "release_id": id.to_string(),
        "status": "SUPERSEDED",
    });
    let _ = nats.publish(subject, payload.to_string().into()).await;

    let _ = nats
        .publish(
            "forest.intent.evaluate",
            state.release_intent_id.to_string().into(),
        )
        .await;
}

/// The pending queue for one target key, newest last.
///
/// `LEFT JOIN` on the annotation so a release forest has no annotation for
/// still appears — it takes part in the collapse, it just has no branch to
/// group by.
async fn pending_for_target(
    event_store: &ReleaseEventStore,
    project_id: Uuid,
    destination_id: Uuid,
) -> anyhow::Result<Vec<Pending>> {
    let rows = sqlx::query!(
        r#"SELECT
              rs.release_id,
              rs.queued_at,
              (SELECT a.ref ->> 'commit_branch'
                 FROM annotations a
                WHERE a.artifact_id = rs.artifact_id
                ORDER BY a.created DESC
                LIMIT 1) as "branch?"
           FROM release_states rs
          WHERE rs.project_id = $1
            AND rs.destination_id = $2
            AND rs.status = 'QUEUED'
          ORDER BY rs.queued_at ASC, rs.release_id ASC"#,
        project_id,
        destination_id,
    )
    .fetch_all(&event_store.db)
    .await
    .context("load pending queue for target")?;

    Ok(rows
        .into_iter()
        .map(|r| Pending {
            release_id: r.release_id,
            queued_at: r.queued_at,
            branch: r.branch,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    /// Queue of `n` releases one second apart, oldest first, all on `main`.
    fn queue(n: usize) -> Vec<Pending> {
        (0..n)
            .map(|i| Pending {
                release_id: Uuid::now_v7(),
                queued_at: at(i as i64),
                branch: Some("main".into()),
            })
            .collect()
    }

    fn superseded_ids(plan: &Plan) -> Vec<Uuid> {
        plan.supersessions.iter().map(|s| s.superseded).collect()
    }

    // ── Liveness: the latest pending release always deploys ──────────

    #[test]
    fn the_newest_pending_release_is_never_superseded() {
        let q = queue(10);
        let newest = q.last().unwrap().release_id;

        // Whichever release the scheduler happens to be holding.
        for holding in &q {
            let p = plan(holding.release_id, &q, false);
            assert!(
                !superseded_ids(&p).contains(&newest),
                "the newest release must never be named as superseded",
            );
        }
    }

    #[test]
    fn holding_the_newest_release_proceeds_and_retires_the_rest() {
        let q = queue(10);
        let newest = q.last().unwrap().release_id;

        let p = plan(newest, &q, false);

        assert_eq!(p.verdict, Verdict::Proceed);
        assert_eq!(p.supersessions.len(), 9, "the other nine collapse");
        assert!(p.supersessions.iter().all(|s| s.by == newest));
        assert_eq!(p.nudge, None);
    }

    #[test]
    fn holding_an_older_release_supersedes_it_and_points_at_the_newest() {
        let q = queue(10);
        let oldest = q[0].release_id;
        let newest = q.last().unwrap().release_id;

        let p = plan(oldest, &q, false);

        assert_eq!(
            p.verdict,
            Verdict::Superseded,
            "an overtaken release must not deploy",
        );
        assert_eq!(p.nudge, Some(newest), "hand attention to the newest");
        assert!(superseded_ids(&p).contains(&oldest));
    }

    #[test]
    fn a_queue_of_one_collapses_to_nothing() {
        let q = queue(1);
        let p = plan(q[0].release_id, &q, false);

        assert_eq!(p.verdict, Verdict::Proceed);
        assert!(p.supersessions.is_empty(), "nothing to supersede");
    }

    #[test]
    fn a_candidate_that_has_left_the_queue_does_not_proceed() {
        // The row was retired or claimed by another scheduler between the
        // caller reading its status and us reading the queue. Saying "proceed"
        // here would hand the scheduler a release it must not dispatch.
        let q = queue(3);
        let gone = Uuid::now_v7();

        let p = plan(gone, &q, false);

        assert_eq!(p.verdict, Verdict::LeftQueue);
        assert_eq!(
            p.nudge,
            Some(q.last().unwrap().release_id),
            "point the scheduler at what is still pending, so the queue drains",
        );
    }

    #[test]
    fn an_empty_queue_has_nothing_to_dispatch_and_nothing_to_retire() {
        let p = plan(Uuid::now_v7(), &[], false);
        assert_eq!(p.verdict, Verdict::LeftQueue);
        assert_eq!(p.nudge, None);
        assert!(p.supersessions.is_empty());
    }

    // ── Safety: no older release ever wins over a newer one ──────────

    #[test]
    fn every_survivor_is_the_newest_of_its_group() {
        let q = queue(6);
        let p = plan(q[0].release_id, &q, false);

        let retired = superseded_ids(&p);
        let survivors: Vec<Uuid> = q
            .iter()
            .map(|r| r.release_id)
            .filter(|id| !retired.contains(id))
            .collect();

        assert_eq!(
            survivors,
            vec![q.last().unwrap().release_id],
            "exactly the newest survives",
        );
    }

    #[test]
    fn a_release_is_only_ever_superseded_by_a_strictly_newer_one() {
        let q = queue(8);
        let by_id: HashMap<Uuid, &Pending> = q.iter().map(|p| (p.release_id, p)).collect();

        let p = plan(q[3].release_id, &q, false);

        for s in &p.supersessions {
            let victim = by_id[&s.superseded];
            let winner = by_id[&s.by];
            assert!(
                (winner.queued_at, winner.release_id) > (victim.queued_at, victim.release_id),
                "a release may only be superseded by one strictly newer",
            );
        }
    }

    #[test]
    fn ties_on_queued_at_are_broken_by_release_id_not_left_ambiguous() {
        let mut a = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(0),
            branch: None,
        };
        let mut b = a.clone();
        b.release_id = Uuid::now_v7();
        if a.release_id > b.release_id {
            std::mem::swap(&mut a, &mut b);
        }
        let q = vec![b.clone(), a.clone()]; // deliberately out of order

        let p = plan(a.release_id, &q, false);

        assert_eq!(superseded_ids(&p), vec![a.release_id]);
        assert_eq!(p.nudge, Some(b.release_id), "the larger id is the newer");
    }

    #[test]
    fn the_decision_does_not_depend_on_the_order_rows_arrive_in() {
        let q = queue(5);
        let mut shuffled = q.clone();
        shuffled.reverse();

        let a = plan(q[0].release_id, &q, false);
        let b = plan(q[0].release_id, &shuffled, false);

        assert_eq!(a, b, "the plan is a function of the set, not the row order");
    }

    // ── Idempotence ──────────────────────────────────────────────────

    #[test]
    fn replanning_after_the_collapse_is_a_no_op() {
        let q = queue(5);
        let newest = q.last().unwrap().release_id;

        let first = plan(newest, &q, false);
        assert_eq!(first.supersessions.len(), 4);

        // What the next scheduler pass sees: only the survivor is still QUEUED.
        let remaining: Vec<Pending> = q.into_iter().filter(|p| p.release_id == newest).collect();
        let second = plan(newest, &remaining, false);

        assert!(second.supersessions.is_empty());
        assert_eq!(second.verdict, Verdict::Proceed);
    }

    // ── The same-branch guard ────────────────────────────────────────

    #[test]
    fn same_branch_only_leaves_a_divergent_release_in_the_queue() {
        let main_old = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(0),
            branch: Some("main".into()),
        };
        let hotfix = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(1),
            branch: Some("hotfix/pager".into()),
        };
        let main_new = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(2),
            branch: Some("main".into()),
        };
        let q = vec![main_old.clone(), hotfix.clone(), main_new.clone()];

        let p = plan(main_old.release_id, &q, true);

        assert_eq!(
            superseded_ids(&p),
            vec![main_old.release_id],
            "only the older release on the same branch collapses",
        );
        assert!(
            !superseded_ids(&p).contains(&hotfix.release_id),
            "a hotfix off another branch is not contained in the newer main \
             release, so it must still deploy",
        );
    }

    #[test]
    fn same_branch_only_still_lets_the_overall_newest_survive() {
        // The newest is on a branch of its own: nothing may overtake it, and
        // the main-branch queue behind it still collapses.
        let main_old = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(0),
            branch: Some("main".into()),
        };
        let main_mid = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(1),
            branch: Some("main".into()),
        };
        let hotfix_newest = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(2),
            branch: Some("hotfix/pager".into()),
        };
        let q = vec![main_old.clone(), main_mid.clone(), hotfix_newest.clone()];

        let p = plan(main_old.release_id, &q, true);

        assert_eq!(superseded_ids(&p), vec![main_old.release_id]);
        assert!(!superseded_ids(&p).contains(&hotfix_newest.release_id));
        assert!(!superseded_ids(&p).contains(&main_mid.release_id));
    }

    #[test]
    fn a_release_with_no_recorded_branch_is_never_superseded_on_a_guess() {
        let unknown = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(0),
            branch: None,
        };
        let main_new = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(1),
            branch: Some("main".into()),
        };
        let q = vec![unknown.clone(), main_new.clone()];

        let p = plan(unknown.release_id, &q, true);

        assert!(
            p.supersessions.is_empty(),
            "forest cannot tell whether the newer release contains this one",
        );
        assert_eq!(p.verdict, Verdict::Proceed);
    }

    #[test]
    fn without_the_guard_branches_are_one_queue() {
        let main = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(0),
            branch: Some("main".into()),
        };
        let hotfix = Pending {
            release_id: Uuid::now_v7(),
            queued_at: at(1),
            branch: Some("hotfix/pager".into()),
        };
        let q = vec![main.clone(), hotfix.clone()];

        let p = plan(main.release_id, &q, false);

        assert_eq!(superseded_ids(&p), vec![main.release_id]);
    }
}
