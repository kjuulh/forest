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

pub struct OrganisationMemberWithUsernameRow {
    pub organisation_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct OrganisationWithRoleRow {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub role: String,
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

    pub async fn get_member(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<Option<OrganisationMemberRow>> {
        let row = sqlx::query_as!(
            OrganisationMemberRow,
            r#"
            SELECT organisation_id, user_id, role, created_at, updated_at
            FROM organisation_members
            WHERE organisation_id = $1 AND user_id = $2
            "#,
            organisation_id,
            user_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_member_with_username(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<Option<OrganisationMemberWithUsernameRow>> {
        let row = sqlx::query_as!(
            OrganisationMemberWithUsernameRow,
            r#"
            SELECT om.organisation_id, om.user_id, u.username, om.role, om.created_at, om.updated_at
            FROM organisation_members om
            JOIN users u ON u.id = om.user_id
            WHERE om.organisation_id = $1 AND om.user_id = $2
            "#,
            organisation_id,
            user_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn remove_member(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            "DELETE FROM organisation_members WHERE organisation_id = $1 AND user_id = $2",
            organisation_id,
            user_id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn update_member_role(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> anyhow::Result<OrganisationMemberRow> {
        let row = sqlx::query_as!(
            OrganisationMemberRow,
            r#"
            UPDATE organisation_members
            SET role = $3, updated_at = now()
            WHERE organisation_id = $1 AND user_id = $2
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

    pub async fn list_members(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<OrganisationMemberWithUsernameRow>> {
        let rows = sqlx::query_as!(
            OrganisationMemberWithUsernameRow,
            r#"
            SELECT om.organisation_id, om.user_id, u.username, om.role, om.created_at, om.updated_at
            FROM organisation_members om
            JOIN users u ON u.id = om.user_id
            WHERE om.organisation_id = $1
            ORDER BY om.created_at ASC
            LIMIT $2 OFFSET $3
            "#,
            organisation_id,
            limit,
            offset,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn count_members(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
    ) -> anyhow::Result<i64> {
        let row = sqlx::query_scalar!(
            r#"SELECT count(*) FROM organisation_members WHERE organisation_id = $1"#,
            organisation_id,
        )
        .fetch_one(db)
        .await?;

        Ok(row.unwrap_or(0))
    }

    pub async fn list_organisations_by_user(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        role_filter: Option<&str>,
    ) -> anyhow::Result<Vec<OrganisationWithRoleRow>> {
        let rows = sqlx::query_as!(
            OrganisationWithRoleRow,
            r#"
            SELECT o.id, o.name, o.created_at, o.updated_at, om.role
            FROM organisations o
            JOIN organisation_members om ON om.organisation_id = o.id
            WHERE om.user_id = $1
              AND ($2::text IS NULL OR om.role = $2)
            ORDER BY o.name ASC
            "#,
            user_id,
            role_filter,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
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
