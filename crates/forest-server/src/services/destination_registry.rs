use std::collections::HashMap;

use anyhow::Context;
use forest_models::{Destination, DestinationType};
use uuid::Uuid;

use crate::{State, repositories::error::DbError};

pub struct DestinationRegistry {
    db: sqlx::PgPool,
}

impl DestinationRegistry {
    pub async fn create_destination(
        &self,
        organisation: &str,
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        destination_type: DestinationType,
    ) -> anyhow::Result<()> {
        // Resolve environment name to environment_id
        let env = sqlx::query!(
            "SELECT id FROM environments WHERE organisation = $1 AND name = $2",
            organisation,
            environment,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup environment")?
        .context("environment not found for this organisation")?;

        sqlx::query!(
            "
                INSERT INTO destinations (
                    organisation,
                    name,
                    environment,
                    environment_id,
                    metadata,
                    type_organisation,
                    type_name,
                    type_version
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    $6,
                    $7,
                    $8
                )
                ",
            organisation,
            name,
            environment,
            env.id,
            serde_json::to_value(&metadata)?,
            destination_type.organisation,
            destination_type.name,
            destination_type.version as i32,
        )
        .execute(&self.db)
        .await
        .map_err(DbError::from)?;

        Ok(())
    }

    pub async fn update_destination(
        &self,
        name: &str,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "
                UPDATE destinations
                SET
                    metadata = $1
                WHERE
                    name = $2
                ",
            serde_json::to_value(&metadata)?,
            name,
        )
        .execute(&self.db)
        .await
        .context("update destination (db)")?;

        if res.rows_affected() != 1 {
            anyhow::bail!("update failed, destination was not found")
        }

        Ok(())
    }

    pub async fn delete_destination(&self, name: &str) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "DELETE FROM destinations WHERE name = $1",
            name,
        )
        .execute(&self.db)
        .await
        .context("delete destination (db)")?;

        if res.rows_affected() != 1 {
            anyhow::bail!("delete failed, destination was not found")
        }

        Ok(())
    }

    pub(crate) async fn get(&self, destination_id: &Uuid) -> anyhow::Result<Option<Destination>> {
        let rec = sqlx::query!(
            "
                SELECT
                    organisation,
                    name,
                    metadata,
                    environment,
                    type_organisation,
                    type_name,
                    type_version
                FROM destinations
                WHERE id = $1
                LIMIT 1;
            ",
            destination_id
        )
        .fetch_optional(&self.db)
        .await
        .context("get destination")?;

        let Some(rec) = rec else { return Ok(None) };

        Ok(Some(Destination::new(
            &rec.organisation.to_string(),
            &rec.name,
            &rec.environment,
            serde_json::from_value(rec.metadata).context("metadata is invalid")?,
            forest_models::DestinationType {
                organisation: rec.type_organisation,
                name: rec.type_name,
                version: rec.type_version as usize,
            },
        )))
    }
}

pub trait DestinationRegistryState {
    fn destination_registry(&self) -> DestinationRegistry;
}

impl DestinationRegistryState for State {
    fn destination_registry(&self) -> DestinationRegistry {
        DestinationRegistry {
            db: self.db.clone(),
        }
    }
}
