use sqlx::PgExecutor;
use uuid::Uuid;

use crate::state::State;

pub struct OrganisationRepository {
    db: sqlx::PgPool,
}

// -- Row types ----------------------------------------------------------------

pub struct OrganisationRow {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct OrganisationMemberRow {
    pub organisation_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// -- Repository implementation ------------------------------------------------

impl OrganisationRepository {
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.db
    }

    pub async fn create_organisation(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        name: &str,
    ) -> anyhow::Result<OrganisationRow> {
        let row = sqlx::query_as!(
            OrganisationRow,
            r#"
            INSERT INTO organisations (id, name)
            VALUES ($1, $2)
            RETURNING id, name, created_at, updated_at
            "#,
            id,
            name,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_organisation(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
    ) -> anyhow::Result<Option<OrganisationRow>> {
        let row = sqlx::query_as!(
            OrganisationRow,
            r#"
            SELECT id, name, created_at, updated_at
            FROM organisations
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_organisation_by_name(
        &self,
        db: impl PgExecutor<'_>,
        name: &str,
    ) -> anyhow::Result<Option<OrganisationRow>> {
        let row = sqlx::query_as!(
            OrganisationRow,
            r#"
            SELECT id, name, created_at, updated_at
            FROM organisations
            WHERE name = $1
            "#,
            name,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn search_organisations(
        &self,
        db: impl PgExecutor<'_>,
        query: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<OrganisationRow>> {
        let pattern = format!("%{query}%");
        let rows = sqlx::query_as!(
            OrganisationRow,
            r#"
            SELECT id, name, created_at, updated_at
            FROM organisations
            WHERE name ILIKE $1
            ORDER BY name ASC
            LIMIT $2 OFFSET $3
            "#,
            pattern,
            limit,
            offset,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn count_organisations_search(
        &self,
        db: impl PgExecutor<'_>,
        query: &str,
    ) -> anyhow::Result<i64> {
        let pattern = format!("%{query}%");
        let row = sqlx::query_scalar!(
            r#"SELECT count(*) FROM organisations WHERE name ILIKE $1"#,
            pattern,
        )
        .fetch_one(db)
        .await?;

        Ok(row.unwrap_or(0))
    }

    pub async fn add_member(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> anyhow::Result<OrganisationMemberRow> {
        let row = sqlx::query_as!(
            OrganisationMemberRow,
            r#"
            INSERT INTO organisation_members (organisation_id, user_id, role)
            VALUES ($1, $2, $3)
            RETURNING organisation_id, user_id, role, created_at, updated_at
            "#,
            organisation_id,
            user_id,
            role,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }
}

// -- State trait --------------------------------------------------------------

pub trait OrganisationRepositoryState {
    fn organisation_repository(&self) -> OrganisationRepository;
}

impl OrganisationRepositoryState for State {
    fn organisation_repository(&self) -> OrganisationRepository {
        OrganisationRepository {
            db: self.db.clone(),
        }
    }
}
