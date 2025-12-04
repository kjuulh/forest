use sqlx::PgPool;
use uuid::Uuid;

use crate::state::State;

pub struct ComponentStagingRepository {
    db: PgPool,
}

impl ComponentStagingRepository {
    pub async fn create_staging(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<Uuid> {
        let rec = sqlx::query!(
            r#"
                   INSERT INTO component_staging (
                       name,
                       namespace,
                       version,
                       status
                   ) VALUES (
                       $1,
                       $2,
                       $3,
                       'staged'
                   )
                   RETURNING id
               "#,
            name,
            namespace,
            version
        )
        .fetch_one(&self.db)
        .await?;

        Ok(rec.id)
    }

    pub async fn commit_staging(&self, context: &Uuid) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await?;

        let staging = sqlx::query!(
            r#"
                SELECT
                    name,
                    namespace,
                    version
                FROM
                    component_staging
                WHERE
                    id = $1
            "#,
            context
        )
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query!(
            r#"
                INSERT INTO components (
                    id,
                    name,
                    namespace,
                    version
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4
                )
            "#,
            context,
            staging.name,
            staging.namespace,
            staging.version,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            r#"
                UPDATE component_staging
                SET
                    status = 'committed'
                WHERE
                    id = $1
            "#,
            context,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }
}

pub trait ComponentStagingRepositoryState {
    fn component_staging_repository(&self) -> ComponentStagingRepository;
}

impl ComponentStagingRepositoryState for State {
    fn component_staging_repository(&self) -> ComponentStagingRepository {
        ComponentStagingRepository {
            db: self.db.clone(),
        }
    }
}
