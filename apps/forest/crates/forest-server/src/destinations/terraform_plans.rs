//! Saved plans for `forest/terraform@1` (forest#284).
//!
//! The plan stage of a release pipeline keeps the binary plan it produced, and
//! the deploy stage of the same release intent applies exactly that file. See
//! the `terraform_plans` migration for why, and for why a row is deleted as
//! soon as its apply has been attempted.

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

/// One terraform root's saved plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPlan {
    /// The work dir, relative to the release's temp root — the same path the
    /// deploy stage lays the artifact out at.
    pub work_dir: String,
    pub plan: Vec<u8>,
    pub lock_file: Option<String>,
}

#[derive(Clone)]
pub struct TerraformPlanStore {
    db: PgPool,
}

impl TerraformPlanStore {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Replace whatever this intent had saved for this destination. A plan
    /// stage that is re-run must not leave an older root's plan behind for the
    /// apply to pick up.
    pub async fn save(
        &self,
        release_intent_id: Uuid,
        destination_id: Uuid,
        plans: &[SavedPlan],
    ) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await.context("begin save plans")?;

        sqlx::query!(
            "DELETE FROM terraform_plans WHERE release_intent_id = $1 AND destination_id = $2",
            release_intent_id,
            destination_id,
        )
        .execute(&mut *tx)
        .await
        .context("clear previous plans")?;

        for plan in plans {
            sqlx::query!(
                r#"
                    INSERT INTO terraform_plans
                        (release_intent_id, destination_id, work_dir, plan, lock_file)
                    VALUES ($1, $2, $3, $4, $5)
                "#,
                release_intent_id,
                destination_id,
                plan.work_dir,
                plan.plan,
                plan.lock_file,
            )
            .execute(&mut *tx)
            .await
            .context("insert plan")?;
        }

        // Plans nobody applied — a rejected stage, an abandoned pipeline — hold
        // secrets and are never going to be valid again. Pruning here keeps it
        // to one code path and no extra job.
        sqlx::query!("DELETE FROM terraform_plans WHERE created_at < now() - interval '7 days'")
            .execute(&mut *tx)
            .await
            .context("prune old plans")?;

        tx.commit().await.context("commit save plans")?;
        Ok(())
    }

    pub async fn load(
        &self,
        release_intent_id: Uuid,
        destination_id: Uuid,
    ) -> anyhow::Result<Vec<SavedPlan>> {
        let rows = sqlx::query!(
            r#"
                SELECT work_dir, plan, lock_file
                FROM terraform_plans
                WHERE release_intent_id = $1 AND destination_id = $2
                ORDER BY work_dir
            "#,
            release_intent_id,
            destination_id,
        )
        .fetch_all(&self.db)
        .await
        .context("load plans")?;

        Ok(rows
            .into_iter()
            .map(|r| SavedPlan {
                work_dir: r.work_dir,
                plan: r.plan,
                lock_file: r.lock_file,
            })
            .collect())
    }

    /// Drop the plans once their apply has been attempted, whether it worked
    /// or not: a plan that applied is spent, and one that failed part-way is
    /// stale — terraform would refuse it anyway.
    pub async fn discard(
        &self,
        release_intent_id: Uuid,
        destination_id: Uuid,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            "DELETE FROM terraform_plans WHERE release_intent_id = $1 AND destination_id = $2",
            release_intent_id,
            destination_id,
        )
        .execute(&self.db)
        .await
        .context("discard plans")?;
        Ok(())
    }

    /// Whether this plan release belongs to a destroy plan stage (forest#286).
    ///
    /// Read from the intent's own pipeline definition rather than carried on
    /// the release, so the only way to get a destroy plan is a stage that says
    /// `destroy: true` — which `validate_pipeline` refuses to combine with
    /// auto-approval. A release with no stage (a direct release) is never one.
    pub async fn is_destroy_plan(&self, release_id: Uuid) -> anyhow::Result<bool> {
        let row = sqlx::query!(
            r#"
                SELECT rs.stage_id, ri.stages
                FROM release_states rs
                JOIN release_intents ri ON ri.id = rs.release_intent_id
                WHERE rs.release_id = $1
            "#,
            release_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("look up the release's stage")?;

        let Some(row) = row else { return Ok(false) };
        let (Some(stage_id), Some(stages)) = (row.stage_id, row.stages) else {
            return Ok(false);
        };
        stage_is_destroy(&stages, &stage_id)
    }

    /// Whether this intent ran a plan stage against this destination. Used to
    /// tell "no plan was ever made" (a direct release: plan and apply in one
    /// go, as before) from "a plan was made and approved but is gone" — which
    /// must fail rather than quietly re-plan and apply something unreviewed.
    pub async fn plan_stage_ran(
        &self,
        release_intent_id: Uuid,
        destination_id: Uuid,
    ) -> anyhow::Result<bool> {
        let ran = sqlx::query_scalar!(
            r#"
                SELECT EXISTS (
                    SELECT 1 FROM release_states
                    WHERE release_intent_id = $1 AND destination_id = $2 AND mode = 'plan'
                ) AS "ran!"
            "#,
            release_intent_id,
            destination_id,
        )
        .fetch_one(&self.db)
        .await
        .context("look up plan stage")?;
        Ok(ran)
    }
}

/// Whether `stage_id` in a stored pipeline definition is a destroy plan.
fn stage_is_destroy(stages: &serde_json::Value, stage_id: &str) -> anyhow::Result<bool> {
    use crate::services::release_pipeline::{PipelineStages, StageConfig};

    let stages: PipelineStages =
        serde_json::from_value(stages.clone()).context("parse the intent's pipeline stages")?;
    Ok(matches!(
        stages.get(stage_id).map(|s| &s.config),
        Some(StageConfig::Plan { destroy: true, .. })
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> TerraformPlanStore {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL — see apps/forest/CLAUDE.md");
        let db = PgPool::connect(&url).await.expect("connect");
        TerraformPlanStore::new(db)
    }

    fn plan(dir: &str, bytes: &[u8]) -> SavedPlan {
        SavedPlan {
            work_dir: dir.into(),
            plan: bytes.to_vec(),
            lock_file: Some(format!("# lock for {dir}")),
        }
    }

    #[tokio::test]
    async fn a_saved_plan_round_trips_byte_for_byte() {
        let store = store().await;
        let (intent, dest) = (Uuid::now_v7(), Uuid::now_v7());
        // Not UTF-8 on purpose: a plan file is a zip.
        let bytes = [0x50, 0x4b, 0x03, 0x04, 0xff, 0x00, 0xfe];

        store
            .save(intent, dest, &[plan("dev/x/forest/terraform@1", &bytes)])
            .await
            .unwrap();

        assert_eq!(
            store.load(intent, dest).await.unwrap(),
            vec![plan("dev/x/forest/terraform@1", &bytes)]
        );
    }

    #[tokio::test]
    async fn saving_again_replaces_every_root_not_just_matching_ones() {
        // A re-run plan stage whose artifact renders fewer roots must not
        // leave the old extra root's plan for the apply to find.
        let store = store().await;
        let (intent, dest) = (Uuid::now_v7(), Uuid::now_v7());

        store
            .save(intent, dest, &[plan("a", b"1"), plan("b", b"1")])
            .await
            .unwrap();
        store.save(intent, dest, &[plan("a", b"2")]).await.unwrap();

        assert_eq!(
            store.load(intent, dest).await.unwrap(),
            vec![plan("a", b"2")]
        );
    }

    #[tokio::test]
    async fn plans_are_scoped_to_their_intent_and_destination() {
        let store = store().await;
        let (intent, dest, other) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

        store.save(intent, dest, &[plan("a", b"1")]).await.unwrap();

        assert!(store.load(intent, other).await.unwrap().is_empty());
        assert!(store.load(other, dest).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn discard_removes_them() {
        let store = store().await;
        let (intent, dest) = (Uuid::now_v7(), Uuid::now_v7());

        store.save(intent, dest, &[plan("a", b"1")]).await.unwrap();
        store.discard(intent, dest).await.unwrap();

        assert!(store.load(intent, dest).await.unwrap().is_empty());
    }

    #[test]
    fn only_a_plan_stage_marked_destroy_is_a_destroy_plan() {
        let stages = serde_json::json!({
            "plan-dev":    {"type": "plan", "depends_on": [], "environment": "dev"},
            "destroy-dev": {"type": "plan", "depends_on": [], "environment": "dev", "destroy": true},
            "deploy-dev":  {"type": "deploy", "depends_on": ["destroy-dev"], "environment": "dev"},
        });
        assert!(stage_is_destroy(&stages, "destroy-dev").unwrap());
        assert!(!stage_is_destroy(&stages, "plan-dev").unwrap());
        assert!(!stage_is_destroy(&stages, "deploy-dev").unwrap());
        assert!(!stage_is_destroy(&stages, "no-such-stage").unwrap());
    }

    #[tokio::test]
    async fn a_release_that_does_not_exist_is_not_a_destroy() {
        let store = store().await;
        assert!(!store.is_destroy_plan(Uuid::now_v7()).await.unwrap());
    }

    #[tokio::test]
    async fn no_plan_stage_is_recorded_for_an_unknown_intent() {
        let store = store().await;
        assert!(
            !store
                .plan_stage_ran(Uuid::now_v7(), Uuid::now_v7())
                .await
                .unwrap()
        );
    }
}
