use std::collections::HashMap;

use anyhow::Context;
use uuid::Uuid;

use super::{
    policy::PolicyRegistry,
    release_event_store::{EventPayload, ReleaseEventStore, ReleaseEventType},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub release_id: Uuid,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supersession {
    pub superseded: Uuid,
    pub by: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Proceed,
    Superseded,
    LeftQueue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub supersessions: Vec<Supersession>,
    pub verdict: Verdict,
    pub nudge: Option<Uuid>,
}

pub fn plan(candidate: Uuid, queue: &[Pending], same_branch_only: bool) -> Plan {
    let mut ordered: Vec<&Pending> = queue.iter().collect();
    ordered.sort_by(|a, b| {
        a.queued_at
            .cmp(&b.queued_at)
            .then_with(|| a.release_id.cmp(&b.release_id))
    });

    let mut newest_per_group: HashMap<GroupKey<'_>, Uuid> = HashMap::new();
    for p in &ordered {
        let key = group_key(p, same_branch_only);
        newest_per_group.insert(key, p.release_id);
    }

    let mut supersessions = Vec::new();
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
            continue;
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

    if verdict == Verdict::LeftQueue {
        nudge = ordered.last().map(|p| p.release_id);
    }

    Plan {
        supersessions,
        verdict,
        nudge,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GroupKey<'a> {
    All,
    Branch(&'a str),
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

    let queue = pending_for_target(event_store, project_id, destination_id, candidate).await?;
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

    if let Some(winner) = plan.nudge {
        let _ = nats
            .publish("forest.release.queued", winner.to_string().into())
            .await;
    }

    Ok(plan.verdict)
}

pub(crate) async fn notify_superseded(
    event_store: &ReleaseEventStore,
    nats: &async_nats::Client,
    id: Uuid,
) {
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

async fn pending_for_target(
    event_store: &ReleaseEventStore,
    project_id: Uuid,
    destination_id: Uuid,
    candidate: Uuid,
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
            AND rs.mode = (SELECT c.mode FROM release_states c WHERE c.release_id = $3)
          ORDER BY rs.queued_at ASC, rs.release_id ASC"#,
        project_id,
        destination_id,
        candidate,
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

    #[test]
    fn the_newest_pending_release_is_never_superseded() {
        let q = queue(10);
        let newest = q.last().unwrap().release_id;

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
        let q = vec![b.clone(), a.clone()];

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

    #[test]
    fn replanning_after_the_collapse_is_a_no_op() {
        let q = queue(5);
        let newest = q.last().unwrap().release_id;

        let first = plan(newest, &q, false);
        assert_eq!(first.supersessions.len(), 4);

        let remaining: Vec<Pending> = q.into_iter().filter(|p| p.release_id == newest).collect();
        let second = plan(newest, &remaining, false);

        assert!(second.supersessions.is_empty());
        assert_eq!(second.verdict, Verdict::Proceed);
    }

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
