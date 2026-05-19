use sqlx::PgPool;

use crate::{services::component_registry::models::ComponentVersion, state::State};

pub struct ComponentsRepository {
    db: PgPool,
}

impl ComponentsRepository {
    pub async fn get_component(
        &self,
        name: &str,
        organisation: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let rec = sqlx::query!(
            r#"
                SELECT
                    id,
                    name,
                    organisation,
                    version
                FROM
                   components
                WHERE
                        name = $1
                    AND organisation = $2
                ORDER BY version DESC
            "#,
            name,
            organisation
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(rec.map(|r| ComponentVersion {
            id: r.id.to_string(),
            name: r.name,
            organisation: r.organisation,
            version: r.version,
        }))
    }

    pub async fn get_component_version(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let rec = sqlx::query!(
            r#"
                SELECT
                    id,
                    name,
                    organisation,
                    version
                FROM
                   components
                WHERE
                        name = $1
                    AND organisation = $2
                    AND version = $3
                ORDER BY version DESC
            "#,
            name,
            organisation,
            version
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(rec.map(|r| ComponentVersion {
            id: r.id.to_string(),
            name: r.name,
            organisation: r.organisation,
            version: r.version,
        }))
    }
}

pub trait ComponentsRepositoryState {
    fn components_repository(&self) -> ComponentsRepository;
}

impl ComponentsRepositoryState for State {
    fn components_repository(&self) -> ComponentsRepository {
        ComponentsRepository {
            db: self.db.clone(),
        }
    }
}
