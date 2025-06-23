use sqlx::PgPool;

use crate::{services::component_registry::models::ComponentVersion, state::State};

pub struct ComponentsRepository {
    db: PgPool,
}

impl ComponentsRepository {
    pub async fn get_component(
        &self,
        name: &str,
        namespace: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let rec = sqlx::query!(
            r#"
                SELECT
                    id,
                    name,
                    namespace,
                    version
                FROM
                   components
                WHERE
                        name = $1
                    AND namespace = $2
                ORDER BY version DESC
            "#,
            name,
            namespace
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(rec.map(|r| ComponentVersion {
            id: r.id.to_string(),
            name: r.name,
            namespace: r.namespace,
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
