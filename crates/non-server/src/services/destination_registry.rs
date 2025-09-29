use anyhow::Context;

use crate::State;

pub struct DestinationRegistry {
    db: sqlx::PgPool,
}

impl DestinationRegistry {
    pub async fn create_destination(&self, name: &str) -> anyhow::Result<()> {
        sqlx::query!(
            "
                INSERT INTO destinations (
                    name,
                    metadata
                ) VALUES (
                    $1,
                    '{}'
                )
                ",
            name
        )
        .execute(&self.db)
        .await
        .context("create destination (db)")?;

        Ok(())
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
