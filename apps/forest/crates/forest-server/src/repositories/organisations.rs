use sqlx::PgExecutor;
use uuid::Uuid;

use super::error::DbError;
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

pub struct AllowedDomainRow {
    pub organisation_id: Uuid,
    pub domain: String,
    pub policy: String,
    pub dns_verification_token: String,
    pub dns_verified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Uuid,
}

pub struct JoinOfferRow {
    pub organisation_id: Uuid,
    pub organisation_name: String,
    pub matched_domain: String,
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
    ) -> Result<OrganisationRow, DbError> {
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
    ) -> Result<OrganisationMemberRow, DbError> {
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
    ) -> Result<(), DbError> {
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
    ) -> Result<OrganisationMemberRow, DbError> {
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

    // -- Allowed-domain auto-invite (DATA-252) --------------------------------

    pub async fn add_allowed_domain(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        domain: &str,
        policy: &str,
        dns_verification_token: &str,
        created_by: Uuid,
    ) -> Result<AllowedDomainRow, DbError> {
        let row = sqlx::query_as!(
            AllowedDomainRow,
            r#"
            INSERT INTO organisation_allowed_domains
                (organisation_id, domain, policy, dns_verification_token, created_by)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING organisation_id, domain, policy, dns_verification_token,
                      dns_verified_at, created_at, created_by
            "#,
            organisation_id,
            domain,
            policy,
            dns_verification_token,
            created_by,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn list_allowed_domains(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
    ) -> anyhow::Result<Vec<AllowedDomainRow>> {
        let rows = sqlx::query_as!(
            AllowedDomainRow,
            r#"
            SELECT organisation_id, domain, policy, dns_verification_token,
                   dns_verified_at, created_at, created_by
            FROM organisation_allowed_domains
            WHERE organisation_id = $1
            ORDER BY domain ASC
            "#,
            organisation_id,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    /// Fetch a single allowed-domain row by (org_id, domain). Used by the
    /// verify-domain service path to retrieve the verification token
    /// before performing the DNS lookup.
    pub async fn get_allowed_domain(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        domain: &str,
    ) -> anyhow::Result<Option<AllowedDomainRow>> {
        let row = sqlx::query_as!(
            AllowedDomainRow,
            r#"
            SELECT organisation_id, domain, policy, dns_verification_token,
                   dns_verified_at, created_at, created_by
            FROM organisation_allowed_domains
            WHERE organisation_id = $1 AND domain = $2
            "#,
            organisation_id,
            domain,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    /// Mark an allowed-domain row as DNS-verified. Idempotent — repeat
    /// calls don't clobber the original verified_at timestamp; only the
    /// first successful verification sets it. Returns true if the row
    /// transitioned (or stayed) verified after this call.
    pub async fn mark_dns_verified(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        domain: &str,
    ) -> Result<bool, DbError> {
        let res = sqlx::query!(
            r#"
            UPDATE organisation_allowed_domains
            SET dns_verified_at = now()
            WHERE organisation_id = $1
              AND domain = $2
              AND dns_verified_at IS NULL
            "#,
            organisation_id,
            domain,
        )
        .execute(db)
        .await?;

        Ok(res.rows_affected() > 0)
    }

    pub async fn remove_allowed_domain(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        domain: &str,
    ) -> Result<bool, DbError> {
        let res = sqlx::query!(
            "DELETE FROM organisation_allowed_domains
             WHERE organisation_id = $1 AND domain = $2",
            organisation_id,
            domain,
        )
        .execute(db)
        .await?;

        Ok(res.rows_affected() > 0)
    }

    /// Compute the join offers for a user: orgs they're not already in
    /// where at least one of their verified emails matches an allowed
    /// domain whose policy actively grants offers (i.e. not 'manual_only')
    /// AND whose DNS ownership has been proven (`dns_verified_at IS NOT
    /// NULL`).
    ///
    /// Domain extraction uses the **rightmost** '@' (`substring(... from
    /// '@([^@]+)$')`), matching Rust's `rsplit_once('@')`. This is the
    /// belt-and-suspenders defense against a malformed multi-`@` email
    /// sneaking past upstream validation: even if `a@b@c.com` is stored,
    /// the extracted domain is `c.com` (the deliverable mailbox), not the
    /// attacker-controlled middle segment.
    pub async fn list_join_offers(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<JoinOfferRow>> {
        let rows = sqlx::query_as!(
            JoinOfferRow,
            r#"
            SELECT DISTINCT ON (o.id)
                   o.id   AS organisation_id,
                   o.name AS organisation_name,
                   oad.domain AS matched_domain
            FROM organisation_allowed_domains oad
            JOIN organisations o ON o.id = oad.organisation_id
            JOIN user_emails ue
              ON LOWER(substring(ue.email from '@([^@]+)$')) = oad.domain
            WHERE ue.user_id = $1
              AND ue.verified = TRUE
              AND oad.policy <> 'manual_only'
              AND oad.dns_verified_at IS NOT NULL
              AND NOT EXISTS (
                  SELECT 1 FROM organisation_members om
                  WHERE om.organisation_id = oad.organisation_id
                    AND om.user_id = $1
              )
            ORDER BY o.id, oad.domain ASC
            "#,
            user_id,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    /// Single-org variant: re-checks eligibility for the accept-offer path.
    /// Returns the matched domain if eligible, None otherwise. Used so
    /// the accept handler can't grant access based on a stale offer.
    /// Same domain-extraction and DNS-verified-gate logic as
    /// `list_join_offers`.
    pub async fn find_join_offer(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        organisation_id: Uuid,
    ) -> anyhow::Result<Option<JoinOfferRow>> {
        let row = sqlx::query_as!(
            JoinOfferRow,
            r#"
            SELECT o.id   AS organisation_id,
                   o.name AS organisation_name,
                   oad.domain AS matched_domain
            FROM organisation_allowed_domains oad
            JOIN organisations o ON o.id = oad.organisation_id
            JOIN user_emails ue
              ON LOWER(substring(ue.email from '@([^@]+)$')) = oad.domain
            WHERE ue.user_id = $1
              AND oad.organisation_id = $2
              AND ue.verified = TRUE
              AND oad.policy <> 'manual_only'
              AND oad.dns_verified_at IS NOT NULL
              AND NOT EXISTS (
                  SELECT 1 FROM organisation_members om
                  WHERE om.organisation_id = oad.organisation_id
                    AND om.user_id = $1
              )
            LIMIT 1
            "#,
            user_id,
            organisation_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
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
