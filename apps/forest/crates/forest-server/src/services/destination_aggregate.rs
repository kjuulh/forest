use std::collections::HashMap;

use anyhow::Context;
use forest_event_store::EventStore;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domains::destination::{self, CreateDestinationParams, DestinationAggregate};

// ============================================================
// Read-model types
// ============================================================

pub struct DestinationRecord {
    pub id: Uuid,
    pub organisation: String,
    pub name: String,
    pub environment: String,
    pub environment_id: Uuid,
    pub metadata: HashMap<String, String>,
    pub sensitive_keys: Vec<String>,
    pub type_organisation: String,
    pub type_name: String,
    pub type_version: i32,
}

// ============================================================
// Service — orchestrates aggregate + projections
// ============================================================

#[derive(Clone)]
pub struct DestinationAggregateService {
    event_store: EventStore,
    db: PgPool,
}

impl DestinationAggregateService {
    pub fn new(event_store: EventStore, db: PgPool) -> Self {
        Self { event_store, db }
    }

    // ----------------------------------------------------------
    // Commands
    // ----------------------------------------------------------

    pub async fn create_destination(
        &self,
        organisation: &str,
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        sensitive_keys: Vec<String>,
        type_organisation: &str,
        type_name: &str,
        type_version: u32,
    ) -> anyhow::Result<Uuid> {
        // Resolve environment name to environment_id from existing projection
        let env_row =
            sqlx::query("SELECT id FROM environments WHERE organisation = $1 AND name = $2")
                .bind(organisation)
                .bind(environment)
                .fetch_optional(&self.db)
                .await
                .context("lookup environment")?
                .context("environment not found for this organisation")?;
        let env_id: Uuid = env_row.get("id");

        let key = destination::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<DestinationAggregate>(&key)
            .await?;

        let destination_id = DestinationAggregate::create(
            &mut root,
            CreateDestinationParams {
                organisation: organisation.to_string(),
                name: name.to_string(),
                environment: environment.to_string(),
                environment_id: env_id,
                metadata: metadata.clone(),
                sensitive_keys: sensitive_keys.clone(),
                type_organisation: type_organisation.to_string(),
                type_name: type_name.to_string(),
                type_version,
            },
        )?;

        // Persist events + update destinations projection atomically
        let org = organisation.to_string();
        let name_owned = name.to_string();
        let env_name = environment.to_string();
        let env_id_owned = env_id;
        let t_org = type_organisation.to_string();
        let t_name = type_name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO destinations (
                            id, organisation, name, environment, environment_id,
                            metadata, sensitive_keys, type_organisation, type_name,
                            type_version
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                    )
                    .bind(destination_id)
                    .bind(&org)
                    .bind(&name_owned)
                    .bind(&env_name)
                    .bind(env_id_owned)
                    .bind(serde_json::to_value(&metadata).unwrap())
                    .bind(serde_json::to_value(&sensitive_keys).unwrap())
                    .bind(&t_org)
                    .bind(&t_name)
                    .bind(type_version as i32)
                    .execute(&mut **tx)
                    .await
                    .context("insert destination projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(destination_id)
    }

    /// `merge` overlays `metadata` onto what is stored instead of replacing it,
    /// so a caller that names two keys does not delete the rest. The overlay is
    /// computed here rather than in the client because the client cannot see
    /// withheld values and would drop them from the map it sent back.
    pub async fn update_metadata(
        &self,
        organisation: &str,
        name: &str,
        metadata: HashMap<String, String>,
        merge: bool,
    ) -> anyhow::Result<()> {
        let key = destination::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<DestinationAggregate>(&key)
            .await?;

        let metadata = if merge {
            let mut merged = root.state.metadata.clone();
            merged.extend(metadata);
            merged
        } else {
            metadata
        };

        DestinationAggregate::update_metadata(&mut root, metadata.clone())?;

        let org_owned = organisation.to_string();
        let name_owned = name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    let res = sqlx::query(
                        "UPDATE destinations SET metadata = $1
                         WHERE organisation = $2 AND name = $3",
                    )
                    .bind(serde_json::to_value(&metadata).unwrap())
                    .bind(&org_owned)
                    .bind(&name_owned)
                    .execute(&mut **tx)
                    .await
                    .context("update destination projection")?;

                    if res.rows_affected() != 1 {
                        anyhow::bail!("destination projection not found for update");
                    }
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    pub async fn update_sensitive_keys(
        &self,
        organisation: &str,
        name: &str,
        sensitive_keys: Vec<String>,
    ) -> anyhow::Result<()> {
        let key = destination::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<DestinationAggregate>(&key)
            .await?;

        DestinationAggregate::update_sensitive_keys(&mut root, sensitive_keys)?;

        // Read back the normalised set the aggregate settled on, so the
        // projection and the event stream cannot drift.
        let normalised = root.state.sensitive_keys.clone();
        let org_owned = organisation.to_string();
        let name_owned = name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    let res = sqlx::query(
                        "UPDATE destinations SET sensitive_keys = $1
                         WHERE organisation = $2 AND name = $3",
                    )
                    .bind(serde_json::to_value(&normalised).unwrap())
                    .bind(&org_owned)
                    .bind(&name_owned)
                    .execute(&mut **tx)
                    .await
                    .context("update destination sensitive keys projection")?;

                    if res.rows_affected() != 1 {
                        anyhow::bail!("destination projection not found for update");
                    }
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    pub async fn delete_destination(&self, organisation: &str, name: &str) -> anyhow::Result<()> {
        let key = destination::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<DestinationAggregate>(&key)
            .await?;

        DestinationAggregate::delete(&mut root)?;

        let org_owned = organisation.to_string();
        let name_owned = name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    let res = sqlx::query(
                        "DELETE FROM destinations
                         WHERE organisation = $1 AND name = $2",
                    )
                    .bind(&org_owned)
                    .bind(&name_owned)
                    .execute(&mut **tx)
                    .await
                    .context("delete destination projection")?;

                    if res.rows_affected() != 1 {
                        anyhow::bail!("destination projection not found for delete");
                    }
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    // ----------------------------------------------------------
    // Queries (read from projections)
    // ----------------------------------------------------------

    pub async fn get(&self, destination_id: &Uuid) -> anyhow::Result<Option<DestinationRecord>> {
        let row = sqlx::query(
            "SELECT id, organisation, name, metadata, sensitive_keys, environment,
                    environment_id, type_organisation, type_name, type_version
             FROM destinations
             WHERE id = $1
             LIMIT 1",
        )
        .bind(destination_id)
        .fetch_optional(&self.db)
        .await
        .context("get destination")?;

        let Some(row) = row else { return Ok(None) };

        Ok(Some(row_to_record(row)?))
    }

    pub async fn get_by_name(
        &self,
        organisation: &str,
        name: &str,
    ) -> anyhow::Result<Option<DestinationRecord>> {
        let row = sqlx::query(
            "SELECT id, organisation, name, metadata, sensitive_keys, environment,
                    environment_id, type_organisation, type_name, type_version
             FROM destinations
             WHERE organisation = $1 AND name = $2
             LIMIT 1",
        )
        .bind(organisation)
        .bind(name)
        .fetch_optional(&self.db)
        .await
        .context("get destination by name")?;

        let Some(row) = row else { return Ok(None) };

        Ok(Some(row_to_record(row)?))
    }
}

fn row_to_record(row: sqlx::postgres::PgRow) -> anyhow::Result<DestinationRecord> {
    let metadata: serde_json::Value = row.get("metadata");
    let sensitive_keys: serde_json::Value = row.get("sensitive_keys");
    Ok(DestinationRecord {
        id: row.get("id"),
        organisation: row.get("organisation"),
        name: row.get("name"),
        environment: row.get("environment"),
        environment_id: row.get("environment_id"),
        metadata: serde_json::from_value(metadata).context("metadata is invalid")?,
        sensitive_keys: serde_json::from_value(sensitive_keys)
            .context("sensitive_keys is invalid")?,
        type_organisation: row.get("type_organisation"),
        type_name: row.get("type_name"),
        type_version: row.get("type_version"),
    })
}

// ============================================================
// State integration
// ============================================================

pub trait DestinationAggregateServiceState {
    fn destination_aggregate_service(&self) -> DestinationAggregateService;
}

impl DestinationAggregateServiceState for crate::state::State {
    fn destination_aggregate_service(&self) -> DestinationAggregateService {
        DestinationAggregateService::new(self.event_store.clone(), self.db.clone())
    }
}
