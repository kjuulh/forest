use sqlx::{PgExecutor, PgPool, Postgres};
use uuid::Uuid;

use super::error::DbError;
use crate::state::State;

pub struct UserRepository {
    db: PgPool,
}

pub struct UserTx {
    tx: sqlx::Transaction<'static, Postgres>,
}

impl UserTx {
    pub async fn commit(self) -> anyhow::Result<()> {
        self.tx.commit().await?;
        Ok(())
    }

    pub fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        &mut self.tx
    }
}

// ─── Row types ───────────────────────────────────────────────────────

pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub profile_picture_url: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct UserEmailRow {
    pub user_id: Uuid,
    pub email: String,
    pub verified: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct IdentityRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub provider_user_id: String,
    pub provider_email: Option<String>,
    pub provider_data: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct NativeCredentialRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub password_hash: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: Vec<u8>,
    pub info: Option<serde_json::Value>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct PersonalAccessTokenRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_hash: Vec<u8>,
    pub scopes: serde_json::Value,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct NativeMfaRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub mfa_type: String,
    pub secret: Vec<u8>,
    pub verified: bool,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct OAuthStateRow {
    pub id: Uuid,
    pub provider: String,
    pub state: String,
    pub redirect_uri: Option<String>,
    pub data: serde_json::Value,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// ─── Repository implementation ───────────────────────────────────────

impl UserRepository {
    pub async fn begin(&self) -> anyhow::Result<UserTx> {
        Ok(UserTx {
            tx: self.db.begin().await?,
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.db
    }

    // ── Users ────────────────────────────────────────────────────────

    pub async fn create_user(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        username: &str,
    ) -> Result<UserRow, DbError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            INSERT INTO users (id, username)
            VALUES ($1, $2)
            RETURNING id, username, profile_picture_url, created_at, updated_at
            "#,
            id,
            username,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_user_by_id(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
    ) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, username, profile_picture_url, created_at, updated_at
            FROM users
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_user_by_username(
        &self,
        db: impl PgExecutor<'_>,
        username: &str,
    ) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, username, profile_picture_url, created_at, updated_at
            FROM users
            WHERE username = $1
            "#,
            username,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn update_user_username(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        username: &str,
    ) -> Result<UserRow, DbError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            UPDATE users
            SET username = $2, updated_at = now()
            WHERE id = $1
            RETURNING id, username, profile_picture_url, created_at, updated_at
            "#,
            id,
            username,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn update_user_profile_picture_url(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        profile_picture_url: Option<&str>,
    ) -> Result<UserRow, DbError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            UPDATE users
            SET profile_picture_url = $2, updated_at = now()
            WHERE id = $1
            RETURNING id, username, profile_picture_url, created_at, updated_at
            "#,
            id,
            profile_picture_url,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn set_profile_picture_url_if_unset(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        profile_picture_url: &str,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE users
            SET profile_picture_url = $2, updated_at = now()
            WHERE id = $1 AND profile_picture_url IS NULL
            "#,
            id,
            profile_picture_url,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn delete_user(&self, db: impl PgExecutor<'_>, id: Uuid) -> Result<(), DbError> {
        sqlx::query!("DELETE FROM users WHERE id = $1", id)
            .execute(db)
            .await?;

        Ok(())
    }

    pub async fn list_users(
        &self,
        db: impl PgExecutor<'_>,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<UserRow>> {
        let rows = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, username, profile_picture_url, created_at, updated_at
            FROM users
            ORDER BY created_at ASC
            LIMIT $1 OFFSET $2
            "#,
            limit,
            offset,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn search_users(
        &self,
        db: impl PgExecutor<'_>,
        query: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<UserRow>> {
        let rows = sqlx::query_as!(
            UserRow,
            r#"
            SELECT u.id, u.username, u.profile_picture_url, u.created_at, u.updated_at
            FROM users u
            WHERE u.id IN (
                SELECT u2.id
                FROM users u2
                LEFT JOIN user_emails ue ON ue.user_id = u2.id
                WHERE u2.username % $1
                   OR ue.email % $1
            )
            ORDER BY similarity(u.username, $1) DESC
            LIMIT $2 OFFSET $3
            "#,
            query,
            limit,
            offset,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    // ── User emails ─────────────────────────────────────────────────

    pub async fn add_user_email(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        email: &str,
    ) -> Result<UserEmailRow, DbError> {
        self.add_user_email_with_verified(db, user_id, email, false)
            .await
    }

    pub async fn add_user_email_with_verified(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        email: &str,
        verified: bool,
    ) -> Result<UserEmailRow, DbError> {
        let row = sqlx::query_as!(
            UserEmailRow,
            r#"
            INSERT INTO user_emails (user_id, email, verified)
            VALUES ($1, $2, $3)
            RETURNING user_id, email, verified, created_at, updated_at
            "#,
            user_id,
            email,
            verified,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn user_has_verified_email(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        let row = sqlx::query!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM user_emails
                WHERE user_id = $1 AND verified = TRUE
            ) AS "has_verified!"
            "#,
            user_id,
        )
        .fetch_one(db)
        .await?;

        Ok(row.has_verified)
    }

    pub async fn get_user_emails(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<UserEmailRow>> {
        let rows = sqlx::query_as!(
            UserEmailRow,
            r#"
            SELECT user_id, email, verified, created_at, updated_at
            FROM user_emails
            WHERE user_id = $1
            "#,
            user_id,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn get_user_by_email(
        &self,
        db: impl PgExecutor<'_>,
        email: &str,
    ) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            SELECT u.id, u.username, u.profile_picture_url, u.created_at, u.updated_at
            FROM users u
            JOIN user_emails ue ON ue.user_id = u.id
            WHERE ue.email = $1
            "#,
            email,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn verify_user_email(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        email: &str,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE user_emails
            SET verified = true, updated_at = now()
            WHERE user_id = $1 AND email = $2
            "#,
            user_id,
            email,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn delete_user_email(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        email: &str,
    ) -> Result<(), DbError> {
        sqlx::query!(
            "DELETE FROM user_emails WHERE user_id = $1 AND email = $2",
            user_id,
            email,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    // ── Identities ──────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn create_identity(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        user_id: Uuid,
        provider: &str,
        provider_user_id: &str,
        provider_email: Option<&str>,
        provider_data: Option<&serde_json::Value>,
    ) -> Result<IdentityRow, DbError> {
        let row = sqlx::query_as!(
            IdentityRow,
            r#"
            INSERT INTO identities (id, user_id, provider, provider_user_id, provider_email, provider_data)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, user_id, provider, provider_user_id, provider_email, provider_data, created_at, updated_at
            "#,
            id,
            user_id,
            provider,
            provider_user_id,
            provider_email,
            provider_data,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_identities_by_user(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<IdentityRow>> {
        let rows = sqlx::query_as!(
            IdentityRow,
            r#"
            SELECT id, user_id, provider, provider_user_id, provider_email, provider_data, created_at, updated_at
            FROM identities
            WHERE user_id = $1
            "#,
            user_id,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn get_identity_by_provider(
        &self,
        db: impl PgExecutor<'_>,
        provider: &str,
        provider_user_id: &str,
    ) -> anyhow::Result<Option<IdentityRow>> {
        let row = sqlx::query_as!(
            IdentityRow,
            r#"
            SELECT id, user_id, provider, provider_user_id, provider_email, provider_data, created_at, updated_at
            FROM identities
            WHERE provider = $1 AND provider_user_id = $2
            "#,
            provider,
            provider_user_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn delete_identity(&self, db: impl PgExecutor<'_>, id: Uuid) -> Result<(), DbError> {
        sqlx::query!("DELETE FROM identities WHERE id = $1", id)
            .execute(db)
            .await?;

        Ok(())
    }

    pub async fn delete_identity_by_provider(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        provider: &str,
    ) -> Result<(), DbError> {
        sqlx::query!(
            "DELETE FROM identities WHERE user_id = $1 AND provider = $2",
            user_id,
            provider,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    // ── Native credentials ──────────────────────────────────────────

    pub async fn set_native_credential(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        user_id: Uuid,
        password_hash: &[u8],
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            INSERT INTO provider_native_credentials (id, user_id, password_hash)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id) DO UPDATE
            SET password_hash = $3, updated_at = now()
            "#,
            id,
            user_id,
            password_hash,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_native_credential(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Option<NativeCredentialRow>> {
        let row = sqlx::query_as!(
            NativeCredentialRow,
            r#"
            SELECT id, user_id, password_hash, created_at, updated_at
            FROM provider_native_credentials
            WHERE user_id = $1
            "#,
            user_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    // ── Native MFA ───────────────────────────────────────────────────

    pub async fn create_native_mfa(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        user_id: Uuid,
        mfa_type: &str,
        secret: &[u8],
    ) -> Result<NativeMfaRow, DbError> {
        let row = sqlx::query_as!(
            NativeMfaRow,
            r#"
            INSERT INTO provider_native_mfa (id, user_id, type, secret)
            VALUES ($1, $2, $3, $4)
            RETURNING id, user_id, type as "mfa_type", secret, verified, last_used_at, created_at, updated_at
            "#,
            id,
            user_id,
            mfa_type,
            secret,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_native_mfa(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Option<NativeMfaRow>> {
        let row = sqlx::query_as!(
            NativeMfaRow,
            r#"
            SELECT id, user_id, type as "mfa_type", secret, verified, last_used_at, created_at, updated_at
            FROM provider_native_mfa
            WHERE user_id = $1
            "#,
            user_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn verify_native_mfa(&self, db: impl PgExecutor<'_>, id: Uuid) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE provider_native_mfa
            SET verified = true, updated_at = now()
            WHERE id = $1
            "#,
            id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn touch_native_mfa(&self, db: impl PgExecutor<'_>, id: Uuid) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE provider_native_mfa
            SET last_used_at = now(), updated_at = now()
            WHERE id = $1
            "#,
            id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn delete_native_mfa(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> Result<(), DbError> {
        sqlx::query!(
            "DELETE FROM provider_native_mfa WHERE user_id = $1",
            user_id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    // ── Sessions ────────────────────────────────────────────────────

    pub async fn create_session(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        user_id: Uuid,
        token_hash: &[u8],
        info: Option<&serde_json::Value>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<SessionRow, DbError> {
        let row = sqlx::query_as!(
            SessionRow,
            r#"
            INSERT INTO sessions (id, user_id, token_hash, info, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, user_id, token_hash, info, expires_at, revoked_at, created_at, updated_at
            "#,
            id,
            user_id,
            token_hash,
            info,
            expires_at,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_session(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
    ) -> anyhow::Result<Option<SessionRow>> {
        let row = sqlx::query_as!(
            SessionRow,
            r#"
            SELECT id, user_id, token_hash, info, expires_at, revoked_at, created_at, updated_at
            FROM sessions
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_session_by_token_hash(
        &self,
        db: impl PgExecutor<'_>,
        token_hash: &[u8],
    ) -> anyhow::Result<Option<SessionRow>> {
        let row = sqlx::query_as!(
            SessionRow,
            r#"
            SELECT id, user_id, token_hash, info, expires_at, revoked_at, created_at, updated_at
            FROM sessions
            WHERE token_hash = $1 AND revoked_at IS NULL
            "#,
            token_hash,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn revoke_session(&self, db: impl PgExecutor<'_>, id: Uuid) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE sessions
            SET revoked_at = now(), updated_at = now()
            WHERE id = $1
            "#,
            id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn revoke_all_user_sessions(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE sessions
            SET revoked_at = now(), updated_at = now()
            WHERE user_id = $1 AND revoked_at IS NULL
            "#,
            user_id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    // ── OAuth state ─────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn create_oauth_state(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        provider: &str,
        state: &str,
        redirect_uri: Option<&str>,
        data: &serde_json::Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<OAuthStateRow, DbError> {
        let row = sqlx::query_as!(
            OAuthStateRow,
            r#"
            INSERT INTO provider_oauth_states (id, provider, state, redirect_uri, data, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, provider, state, redirect_uri, data, expires_at, created_at, updated_at
            "#,
            id,
            provider,
            state,
            redirect_uri,
            data,
            expires_at,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_oauth_state_by_state(
        &self,
        db: impl PgExecutor<'_>,
        state: &str,
    ) -> anyhow::Result<Option<OAuthStateRow>> {
        let row = sqlx::query_as!(
            OAuthStateRow,
            r#"
            SELECT id, provider, state, redirect_uri, data, expires_at, created_at, updated_at
            FROM provider_oauth_states
            WHERE state = $1
            "#,
            state,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn delete_oauth_state(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
    ) -> Result<(), DbError> {
        sqlx::query!("DELETE FROM provider_oauth_states WHERE id = $1", id)
            .execute(db)
            .await?;

        Ok(())
    }

    // ── Personal access tokens ──────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn create_personal_access_token(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        user_id: Uuid,
        name: &str,
        token_hash: &[u8],
        scopes: &serde_json::Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<PersonalAccessTokenRow, DbError> {
        let row = sqlx::query_as!(
            PersonalAccessTokenRow,
            r#"
            INSERT INTO personal_access_tokens (id, user_id, name, token_hash, scopes, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, user_id, name, token_hash, scopes, expires_at, last_used, created_at, updated_at
            "#,
            id,
            user_id,
            name,
            token_hash,
            scopes,
            expires_at,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn get_personal_access_token_by_hash(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
        token_hash: &[u8],
    ) -> anyhow::Result<Option<PersonalAccessTokenRow>> {
        let row = sqlx::query_as!(
            PersonalAccessTokenRow,
            r#"
            SELECT id, user_id, name, token_hash, scopes, expires_at, last_used, created_at, updated_at
            FROM personal_access_tokens
            WHERE user_id = $1 AND token_hash = $2
            "#,
            user_id,
            token_hash,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn list_personal_access_tokens(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<PersonalAccessTokenRow>> {
        let rows = sqlx::query_as!(
            PersonalAccessTokenRow,
            r#"
            SELECT id, user_id, name, token_hash, scopes, expires_at, last_used, created_at, updated_at
            FROM personal_access_tokens
            WHERE user_id = $1
            ORDER BY created_at DESC
            "#,
            user_id,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn touch_personal_access_token(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            UPDATE personal_access_tokens
            SET last_used = now(), updated_at = now()
            WHERE id = $1
            "#,
            id,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn delete_personal_access_token(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
    ) -> Result<(), DbError> {
        sqlx::query!("DELETE FROM personal_access_tokens WHERE id = $1", id)
            .execute(db)
            .await?;

        Ok(())
    }

    /// User-scoped delete. Returns the number of rows deleted (0 if the
    /// token doesn't exist or doesn't belong to `user_id`). Used by the
    /// gRPC handler to enforce that callers can only delete their own
    /// tokens without leaking whether an arbitrary `token_id` exists.
    pub async fn delete_personal_access_token_for_user(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<u64, DbError> {
        let result = sqlx::query!(
            "DELETE FROM personal_access_tokens WHERE id = $1 AND user_id = $2",
            id,
            user_id,
        )
        .execute(db)
        .await?;

        Ok(result.rows_affected())
    }
}

// ─── State trait ─────────────────────────────────────────────────────

pub trait UserRepositoryState {
    fn user_repository(&self) -> UserRepository;
}

impl UserRepositoryState for State {
    fn user_repository(&self) -> UserRepository {
        UserRepository {
            db: self.db.clone(),
        }
    }
}
