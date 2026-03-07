use anyhow::Context;
use uuid::Uuid;

use crate::State;

pub struct EnvironmentRecord {
    pub id: Uuid,
    pub organisation: String,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct EnvironmentRegistry {
    db: sqlx::PgPool,
}

impl EnvironmentRegistry {
    pub async fn create(
        &self,
        organisation: &str,
        name: &str,
        description: Option<&str>,
        sort_order: i32,
    ) -> anyhow::Result<EnvironmentRecord> {
        let rec = sqlx::query!(
            r#"
            INSERT INTO environments (organisation, name, description, sort_order)
            VALUES ($1, $2, $3, $4)
            RETURNING id, organisation, name, description, sort_order, created_at
            "#,
            organisation,
            name,
            description,
            sort_order,
        )
        .fetch_one(&self.db)
        .await
        .context("create environment")?;

        Ok(EnvironmentRecord {
            id: rec.id,
            organisation: rec.organisation,
            name: rec.name,
            description: rec.description,
            sort_order: rec.sort_order,
            created_at: rec.created_at,
        })
    }

    pub async fn get_by_id(&self, id: &Uuid) -> anyhow::Result<Option<EnvironmentRecord>> {
        let rec = sqlx::query!(
            r#"
            SELECT id, organisation, name, description, sort_order, created_at
            FROM environments
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(&self.db)
        .await
        .context("get environment by id")?;

        Ok(rec.map(|r| EnvironmentRecord {
            id: r.id,
            organisation: r.organisation,
            name: r.name,
            description: r.description,
            sort_order: r.sort_order,
            created_at: r.created_at,
        }))
    }

    pub async fn get_by_org_name(
        &self,
        organisation: &str,
        name: &str,
    ) -> anyhow::Result<Option<EnvironmentRecord>> {
        let rec = sqlx::query!(
            r#"
            SELECT id, organisation, name, description, sort_order, created_at
            FROM environments
            WHERE organisation = $1 AND name = $2
            "#,
            organisation,
            name,
        )
        .fetch_optional(&self.db)
        .await
        .context("get environment by org+name")?;

        Ok(rec.map(|r| EnvironmentRecord {
            id: r.id,
            organisation: r.organisation,
            name: r.name,
            description: r.description,
            sort_order: r.sort_order,
            created_at: r.created_at,
        }))
    }

    pub async fn list(&self, organisation: &str) -> anyhow::Result<Vec<EnvironmentRecord>> {
        let recs = sqlx::query!(
            r#"
            SELECT id, organisation, name, description, sort_order, created_at
            FROM environments
            WHERE organisation = $1
            ORDER BY sort_order, name
            "#,
            organisation,
        )
        .fetch_all(&self.db)
        .await
        .context("list environments")?;

        Ok(recs
            .into_iter()
            .map(|r| EnvironmentRecord {
                id: r.id,
                organisation: r.organisation,
                name: r.name,
                description: r.description,
                sort_order: r.sort_order,
                created_at: r.created_at,
            })
            .collect())
    }

    pub async fn update(
        &self,
        id: &Uuid,
        description: Option<&str>,
        sort_order: Option<i32>,
    ) -> anyhow::Result<EnvironmentRecord> {
        let rec = sqlx::query!(
            r#"
            UPDATE environments
            SET
                description = COALESCE($2, description),
                sort_order = COALESCE($3, sort_order),
                updated_at = now()
            WHERE id = $1
            RETURNING id, organisation, name, description, sort_order, created_at
            "#,
            id,
            description,
            sort_order,
        )
        .fetch_one(&self.db)
        .await
        .context("update environment")?;

        Ok(EnvironmentRecord {
            id: rec.id,
            organisation: rec.organisation,
            name: rec.name,
            description: rec.description,
            sort_order: rec.sort_order,
            created_at: rec.created_at,
        })
    }

    pub async fn delete(&self, id: &Uuid) -> anyhow::Result<()> {
        let res = sqlx::query!("DELETE FROM environments WHERE id = $1", id)
            .execute(&self.db)
            .await
            .context("delete environment")?;

        if res.rows_affected() == 0 {
            anyhow::bail!("environment not found");
        }

        Ok(())
    }
}

pub trait EnvironmentRegistryState {
    fn environment_registry(&self) -> EnvironmentRegistry;
}

impl EnvironmentRegistryState for State {
    fn environment_registry(&self) -> EnvironmentRegistry {
        EnvironmentRegistry {
            db: self.db.clone(),
        }
    }
}
