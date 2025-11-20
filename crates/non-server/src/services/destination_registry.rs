use std::collections::HashMap;

use anyhow::Context;
use non_models::{Destination, DestinationType};
use uuid::Uuid;

use crate::State;

pub struct DestinationRegistry {
    db: sqlx::PgPool,
}

impl DestinationRegistry {
    pub async fn create_destination(
        &self,
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        destination_type: DestinationType,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            "
                INSERT INTO destinations (
                    name,
                    environment,
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
                    $6

                )
                ",
            name,
            environment,
            serde_json::to_value(&metadata)?,
            destination_type.organisation,
            destination_type.name,
            destination_type.version as i32,
        )
        .execute(&self.db)
        .await
        .context("create destination (db)")?;

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

    pub(crate) async fn get(&self, destination_id: &Uuid) -> anyhow::Result<Option<Destination>> {
        let rec = sqlx::query!(
            "
                SELECT
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
            &rec.name,
            &rec.environment,
            serde_json::from_value(rec.metadata).context("metadata is invalid")?,
            non_models::DestinationType {
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
