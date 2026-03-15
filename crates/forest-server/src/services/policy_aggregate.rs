use anyhow::Context;
use forest_event_store::EventStore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domains::policy::{
    self, CreatePolicyParams, PolicyAggregate, UpdatePolicyConfigParams,
};
use crate::services::policy::PolicyRecord;

// ============================================================
// Service — orchestrates aggregate + projection for writes
// ============================================================

#[derive(Clone)]
pub struct PolicyAggregateService {
    event_store: EventStore,
    db: PgPool,
}

impl PolicyAggregateService {
    pub fn new(event_store: EventStore, db: PgPool) -> Self {
        Self { event_store, db }
    }

    pub async fn create(
        &self,
        project_id: Uuid,
        name: String,
        policy_type: String,
        config: serde_json::Value,
    ) -> anyhow::Result<PolicyRecord> {
        let key = policy::stream_key(&project_id, &name);
        let mut root = self
            .event_store
            .load_or_default::<PolicyAggregate>(&key)
            .await?;

        let policy_id = PolicyAggregate::create(
            &mut root,
            CreatePolicyParams {
                project_id,
                name: name.clone(),
                policy_type: policy_type.clone(),
                config: config.clone(),
            },
        )?;

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO policies (id, project_id, name, policy_type, config)
                         VALUES ($1, $2, $3, $4, $5)",
                    )
                    .bind(policy_id)
                    .bind(project_id)
                    .bind(&name)
                    .bind(&policy_type)
                    .bind(&config)
                    .execute(&mut **tx)
                    .await
                    .context("insert policy projection")?;
                    Ok(())
                })
            })
            .await?;

        self.get_by_name(&project_id, &root.state.name)
            .await?
            .context("policy projection not found after create")
    }

    pub async fn update(
        &self,
        project_id: &Uuid,
        name: &str,
        enabled: Option<bool>,
        config: Option<(String, serde_json::Value)>,
    ) -> anyhow::Result<PolicyRecord> {
        let key = policy::stream_key(project_id, name);
        let mut root = self
            .event_store
            .load_or_default::<PolicyAggregate>(&key)
            .await?;

        if let Some(enabled) = enabled
            && enabled != root.state.enabled
        {
            PolicyAggregate::toggle_enabled(&mut root, enabled)?;
        }

        if let Some((policy_type, config_json)) = config {
            PolicyAggregate::update_config(
                &mut root,
                UpdatePolicyConfigParams {
                    policy_type,
                    config: config_json,
                },
            )?;
        }

        if !root.has_pending() {
            return self
                .get_by_name(project_id, name)
                .await?
                .context("policy not found");
        }

        let project_id_owned = *project_id;
        let name_owned = name.to_string();
        let enabled = root.state.enabled;
        let policy_type = root.state.policy_type.clone();
        let config_val = root.state.config.clone();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "UPDATE policies SET
                            enabled = $3, policy_type = $4, config = $5, updated_at = now()
                        WHERE project_id = $1 AND name = $2",
                    )
                    .bind(project_id_owned)
                    .bind(&name_owned)
                    .bind(enabled)
                    .bind(&policy_type)
                    .bind(&config_val)
                    .execute(&mut **tx)
                    .await
                    .context("update policy projection")?;
                    Ok(())
                })
            })
            .await?;

        self.get_by_name(project_id, name)
            .await?
            .context("policy not found after update")
    }

    pub async fn delete(&self, project_id: &Uuid, name: &str) -> anyhow::Result<()> {
        let key = policy::stream_key(project_id, name);
        let mut root = self
            .event_store
            .load_or_default::<PolicyAggregate>(&key)
            .await?;

        PolicyAggregate::delete(&mut root)?;

        let project_id_owned = *project_id;
        let name_owned = name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    let res = sqlx::query(
                        "DELETE FROM policies WHERE project_id = $1 AND name = $2",
                    )
                    .bind(project_id_owned)
                    .bind(&name_owned)
                    .execute(&mut **tx)
                    .await
                    .context("delete policy projection")?;

                    if res.rows_affected() != 1 {
                        anyhow::bail!("policy projection not found for delete");
                    }
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    async fn get_by_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> anyhow::Result<Option<PolicyRecord>> {
        let rec = sqlx::query_as!(
            PolicyRecord,
            r#"SELECT id, project_id, name, enabled, policy_type, config, created_at, updated_at
            FROM policies
            WHERE project_id = $1 AND name = $2"#,
            project_id,
            name,
        )
        .fetch_optional(&self.db)
        .await
        .context("get policy by name")?;

        Ok(rec)
    }
}

// ============================================================
// State integration
// ============================================================

pub trait PolicyAggregateServiceState {
    fn policy_aggregate_service(&self) -> PolicyAggregateService;
}

impl PolicyAggregateServiceState for crate::state::State {
    fn policy_aggregate_service(&self) -> PolicyAggregateService {
        PolicyAggregateService::new(self.event_store.clone(), self.db.clone())
    }
}
