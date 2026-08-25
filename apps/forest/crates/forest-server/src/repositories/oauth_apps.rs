use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::error::DbError;
use crate::state::State;

/// Repository for organisation-owned OAuth applications.
///
/// OAuth apps are credential records (like personal access tokens), not
/// event-sourced domain aggregates — CRUD goes straight to the projection
/// table. Only the SHA-256 hash of the client_secret is ever persisted.
pub struct OAuthAppRepository {
    db: PgPool,
}

/// A row of the `oauth_apps` table. `client_secret_hash` stays in the
/// repository layer and is never surfaced to gRPC responses.
pub struct OAuthAppRow {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub name: String,
    pub description: String,
    pub homepage_url: String,
    pub client_id: String,
    pub client_secret_hash: Vec<u8>,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    /// Which OAuth grants this app may use. Apps created before
    /// machine-to-machine support default to `authorization_code` only,
    /// so nothing gains a capability by being old.
    pub grant_types: Vec<String>,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A live machine (client-credentials) token: which app it belongs to and
/// what it may do. No user — that is the whole point of the grant.
pub struct ResolvedClientTokenRow {
    pub app_id: Uuid,
    pub organisation_id: Uuid,
    pub scopes: Vec<String>,
}

/// The bearer + scopes resolved from a live access token.
pub struct ResolvedTokenRow {
    pub app_id: Uuid,
    pub user_id: Uuid,
    pub scopes: Vec<String>,
}

/// A token row looked up by refresh hash (any revocation status).
pub struct RefreshLookupRow {
    pub app_id: Uuid,
    pub user_id: Uuid,
    pub scopes: Vec<String>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub refresh_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// One live token row joined to its app, for the authorized-apps view.
pub struct GrantRow {
    pub app_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The binding returned when an authorization code is consumed.
pub struct ConsumedCodeRow {
    pub app_id: Uuid,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub nonce: Option<String>,
}

impl OAuthAppRepository {
    /// Construct directly from a pool (used by the reaper component and tests;
    /// normal callers use `OAuthAppRepositoryState::oauth_app_repository`).
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub fn pool(&self) -> &PgPool {
        &self.db
    }

    /// Delete dead OAuth rows: expired/consumed authorization codes, and access
    /// tokens that are fully dead (refresh — or access, when there is no
    /// refresh — expired) or revoked more than 7 days ago. Revoked-but-recent
    /// rows are kept so refresh-token reuse detection still has a signal.
    /// Returns `(codes_deleted, tokens_deleted)`. Pure housekeeping: these rows
    /// never resolve anyway (reads filter on `expires_at`/`revoked_at`).
    pub async fn reap_expired(&self, db: impl PgExecutor<'_> + Copy) -> anyhow::Result<(u64, u64)> {
        let codes = sqlx::query!(
            "DELETE FROM oauth_authorization_codes WHERE expires_at < now() OR consumed_at IS NOT NULL",
        )
        .execute(db)
        .await?
        .rows_affected();

        let tokens = sqlx::query!(
            r#"
            DELETE FROM oauth_access_tokens
            WHERE COALESCE(refresh_expires_at, expires_at) < now()
               OR (revoked_at IS NOT NULL AND revoked_at < now() - interval '7 days')
            "#,
        )
        .execute(db)
        .await?
        .rows_affected();

        Ok((codes, tokens))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_oauth_app(
        &self,
        db: impl PgExecutor<'_>,
        id: Uuid,
        organisation_id: Uuid,
        name: &str,
        description: &str,
        homepage_url: &str,
        client_id: &str,
        client_secret_hash: &[u8],
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
        created_by: Uuid,
    ) -> Result<OAuthAppRow, DbError> {
        let row = sqlx::query_as!(
            OAuthAppRow,
            r#"
            INSERT INTO oauth_apps
                (id, organisation_id, name, description, homepage_url, client_id,
                 client_secret_hash, redirect_uris, scopes, grant_types, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, organisation_id, name, description, homepage_url, client_id,
                      client_secret_hash, redirect_uris, scopes, grant_types, created_by, created_at, updated_at
            "#,
            id,
            organisation_id,
            name,
            description,
            homepage_url,
            client_id,
            client_secret_hash,
            redirect_uris,
            scopes,
            grant_types,
            created_by,
        )
        .fetch_one(db)
        .await?;

        Ok(row)
    }

    pub async fn list_oauth_apps(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
    ) -> anyhow::Result<Vec<OAuthAppRow>> {
        let rows = sqlx::query_as!(
            OAuthAppRow,
            r#"
            SELECT id, organisation_id, name, description, homepage_url, client_id,
                   client_secret_hash, redirect_uris, scopes, grant_types, created_by, created_at, updated_at
            FROM oauth_apps
            WHERE organisation_id = $1
            ORDER BY created_at DESC
            "#,
            organisation_id,
        )
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    /// Fetch a single app scoped to an organisation (the org-settings path).
    pub async fn get_oauth_app(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        app_id: Uuid,
    ) -> anyhow::Result<Option<OAuthAppRow>> {
        let row = sqlx::query_as!(
            OAuthAppRow,
            r#"
            SELECT id, organisation_id, name, description, homepage_url, client_id,
                   client_secret_hash, redirect_uris, scopes, grant_types, created_by, created_at, updated_at
            FROM oauth_apps
            WHERE organisation_id = $1 AND id = $2
            "#,
            organisation_id,
            app_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    /// Fetch by public client_id (the authorize/token path; not org-scoped).
    pub async fn get_oauth_app_by_client_id(
        &self,
        db: impl PgExecutor<'_>,
        client_id: &str,
    ) -> anyhow::Result<Option<OAuthAppRow>> {
        let row = sqlx::query_as!(
            OAuthAppRow,
            r#"
            SELECT id, organisation_id, name, description, homepage_url, client_id,
                   client_secret_hash, redirect_uris, scopes, grant_types,
                   created_by, created_at, updated_at
            FROM oauth_apps
            WHERE client_id = $1
            "#,
            client_id,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    // ── Machine (client-credentials) tokens ──────────────────────────
    //
    // Deliberately a separate table from `oauth_access_tokens`: these
    // have no user, no refresh token and no consent, and keeping them
    // apart means neither flow can accept the other's tokens. See the
    // migration for the full reasoning.

    pub async fn insert_client_token(
        &self,
        db: impl PgExecutor<'_>,
        token_hash: &[u8],
        app_id: Uuid,
        scopes: &[String],
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            INSERT INTO oauth_client_tokens (token_hash, app_id, scopes, expires_at)
            VALUES ($1, $2, $3, $4)
            "#,
            token_hash,
            app_id,
            scopes,
            expires_at,
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Resolve a live machine token to its app + granted scopes, touching
    /// `last_used_at`. `None` when unknown, expired or revoked — the
    /// caller cannot tell which, on purpose.
    pub async fn resolve_client_token(
        &self,
        db: impl PgExecutor<'_>,
        token_hash: &[u8],
    ) -> anyhow::Result<Option<ResolvedClientTokenRow>> {
        let row = sqlx::query_as!(
            ResolvedClientTokenRow,
            r#"
            UPDATE oauth_client_tokens t
            SET last_used_at = now()
            FROM oauth_apps a
            WHERE t.token_hash = $1
              AND t.app_id = a.id
              AND t.revoked_at IS NULL
              AND t.expires_at > now()
            RETURNING t.app_id, a.organisation_id, t.scopes
            "#,
            token_hash,
        )
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    /// Revoke every live machine token for an app. Used when the app's
    /// secret is rotated or the app is disabled.
    pub async fn revoke_client_tokens_for_app(
        &self,
        db: impl PgExecutor<'_>,
        app_id: Uuid,
    ) -> Result<u64, DbError> {
        let done = sqlx::query!(
            r#"
            UPDATE oauth_client_tokens
            SET revoked_at = now()
            WHERE app_id = $1 AND revoked_at IS NULL
            "#,
            app_id,
        )
        .execute(db)
        .await?;
        Ok(done.rows_affected())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_oauth_app(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        app_id: Uuid,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
    ) -> Result<Option<OAuthAppRow>, DbError> {
        let row = sqlx::query_as!(
            OAuthAppRow,
            r#"
            UPDATE oauth_apps
            SET name = $3, description = $4, homepage_url = $5,
                redirect_uris = $6, scopes = $7, grant_types = $8, updated_at = now()
            WHERE organisation_id = $1 AND id = $2
            RETURNING id, organisation_id, name, description, homepage_url, client_id,
                      client_secret_hash, redirect_uris, scopes, grant_types, created_by, created_at, updated_at
            "#,
            organisation_id,
            app_id,
            name,
            description,
            homepage_url,
            redirect_uris,
            scopes,
            grant_types,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn rotate_oauth_app_secret(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        app_id: Uuid,
        client_secret_hash: &[u8],
    ) -> Result<Option<OAuthAppRow>, DbError> {
        let row = sqlx::query_as!(
            OAuthAppRow,
            r#"
            UPDATE oauth_apps
            SET client_secret_hash = $3, updated_at = now()
            WHERE organisation_id = $1 AND id = $2
            RETURNING id, organisation_id, name, description, homepage_url, client_id,
                      client_secret_hash, redirect_uris, scopes, grant_types, created_by, created_at, updated_at
            "#,
            organisation_id,
            app_id,
            client_secret_hash,
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    // ── Authorization codes ──────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn create_authorization_code(
        &self,
        db: impl PgExecutor<'_>,
        code_hash: &[u8],
        app_id: Uuid,
        user_id: Uuid,
        redirect_uri: &str,
        scopes: &[String],
        code_challenge: Option<&str>,
        code_challenge_method: Option<&str>,
        nonce: Option<&str>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            INSERT INTO oauth_authorization_codes
                (code_hash, app_id, user_id, redirect_uri, scopes,
                 code_challenge, code_challenge_method, nonce, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            code_hash,
            app_id,
            user_id,
            redirect_uri,
            scopes,
            code_challenge,
            code_challenge_method,
            nonce,
            expires_at,
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Atomically mark a code consumed and return its binding. Returns `None`
    /// if the code is missing, expired, or already consumed (replay) — the
    /// single-use guarantee lives in the `consumed_at IS NULL` predicate.
    pub async fn consume_authorization_code(
        &self,
        db: impl PgExecutor<'_>,
        code_hash: &[u8],
    ) -> anyhow::Result<Option<ConsumedCodeRow>> {
        let row = sqlx::query_as!(
            ConsumedCodeRow,
            r#"
            UPDATE oauth_authorization_codes
            SET consumed_at = now()
            WHERE code_hash = $1 AND consumed_at IS NULL AND expires_at > now()
            RETURNING app_id, user_id, redirect_uri, scopes,
                      code_challenge, code_challenge_method, nonce
            "#,
            code_hash,
        )
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    // ── Access tokens ─────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_access_token(
        &self,
        db: impl PgExecutor<'_>,
        token_hash: &[u8],
        app_id: Uuid,
        user_id: Uuid,
        scopes: &[String],
        refresh_hash: Option<&[u8]>,
        expires_at: chrono::DateTime<chrono::Utc>,
        refresh_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), DbError> {
        sqlx::query!(
            r#"
            INSERT INTO oauth_access_tokens
                (token_hash, app_id, user_id, scopes, refresh_hash,
                 expires_at, refresh_expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            token_hash,
            app_id,
            user_id,
            scopes,
            refresh_hash,
            expires_at,
            refresh_expires_at,
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Resolve a live (unexpired, unrevoked) access token to its bearer and
    /// granted scopes, touching `last_used_at`. Returns `None` if the token is
    /// unknown, expired, or revoked.
    pub async fn resolve_access_token(
        &self,
        db: impl PgExecutor<'_>,
        token_hash: &[u8],
    ) -> anyhow::Result<Option<ResolvedTokenRow>> {
        let row = sqlx::query_as!(
            ResolvedTokenRow,
            r#"
            UPDATE oauth_access_tokens
            SET last_used_at = now()
            WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > now()
            RETURNING app_id, user_id, scopes
            "#,
            token_hash,
        )
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    /// Look up an access-token row by its refresh-token hash, regardless of
    /// revocation status (the caller distinguishes reuse from validity).
    pub async fn find_token_by_refresh(
        &self,
        db: impl PgExecutor<'_>,
        refresh_hash: &[u8],
    ) -> anyhow::Result<Option<RefreshLookupRow>> {
        let row = sqlx::query_as!(
            RefreshLookupRow,
            r#"
            SELECT app_id, user_id, scopes, revoked_at, refresh_expires_at
            FROM oauth_access_tokens
            WHERE refresh_hash = $1
            "#,
            refresh_hash,
        )
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    /// Atomically consume (revoke) a live refresh token for a given client and
    /// return its bearer + scopes. The `revoked_at IS NULL` predicate makes
    /// rotation single-use even under concurrent refreshes — only one caller's
    /// UPDATE can match. Returns `None` if missing, expired, already rotated,
    /// or belonging to another client.
    pub async fn consume_refresh_token(
        &self,
        db: impl PgExecutor<'_>,
        refresh_hash: &[u8],
        app_id: Uuid,
    ) -> anyhow::Result<Option<ResolvedTokenRow>> {
        let row = sqlx::query_as!(
            ResolvedTokenRow,
            r#"
            UPDATE oauth_access_tokens
            SET revoked_at = now()
            WHERE refresh_hash = $1 AND app_id = $2 AND revoked_at IS NULL
              AND (refresh_expires_at IS NULL OR refresh_expires_at > now())
            RETURNING app_id, user_id, scopes
            "#,
            refresh_hash,
            app_id,
        )
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    /// Revoke every live token for a (user, app) grant. Used for refresh-token
    /// reuse defence and for explicit grant revocation. Returns the count.
    pub async fn revoke_user_app_tokens(
        &self,
        db: impl PgExecutor<'_>,
        app_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<u64> {
        let result = sqlx::query!(
            "UPDATE oauth_access_tokens SET revoked_at = now() WHERE app_id = $1 AND user_id = $2 AND revoked_at IS NULL",
            app_id,
            user_id,
        )
        .execute(db)
        .await?;
        Ok(result.rows_affected())
    }

    // ── Remembered consent ────────────────────────────────────────────

    /// Record (or widen) a user's consent for an app. Scopes accumulate: an
    /// `email` consent on top of an existing `profile` consent yields both.
    pub async fn upsert_consent(
        &self,
        db: impl PgExecutor<'_>,
        app_id: Uuid,
        user_id: Uuid,
        scopes: &[String],
    ) -> anyhow::Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO oauth_consents (app_id, user_id, scopes)
            VALUES ($1, $2, $3)
            ON CONFLICT (app_id, user_id) DO UPDATE SET
                scopes = (
                    SELECT array_agg(DISTINCT s)
                    FROM unnest(oauth_consents.scopes || EXCLUDED.scopes) AS s
                ),
                updated_at = now()
            "#,
            app_id,
            user_id,
            scopes,
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// The scopes a user has consented to for a client (by client_id), or
    /// `None` if there is no consent on record.
    pub async fn get_consent_by_client(
        &self,
        db: impl PgExecutor<'_>,
        client_id: &str,
        user_id: Uuid,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let row = sqlx::query_scalar!(
            r#"
            SELECT c.scopes
            FROM oauth_consents c
            JOIN oauth_apps a ON a.id = c.app_id
            WHERE a.client_id = $1 AND c.user_id = $2
            "#,
            client_id,
            user_id,
        )
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    /// Forget a user's consent for an app (called on grant revocation).
    pub async fn delete_consent(
        &self,
        db: impl PgExecutor<'_>,
        app_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            "DELETE FROM oauth_consents WHERE app_id = $1 AND user_id = $2",
            app_id,
            user_id,
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Live (unrevoked, unexpired) token rows for a user, joined to their app,
    /// for building the "authorized apps" view. One row per token; the service
    /// groups by app.
    pub async fn list_live_grant_rows(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<GrantRow>> {
        let rows = sqlx::query_as!(
            GrantRow,
            r#"
            SELECT a.id AS app_id, a.name AS name, t.scopes AS scopes, t.created_at AS created_at
            FROM oauth_access_tokens t
            JOIN oauth_apps a ON a.id = t.app_id
            WHERE t.user_id = $1 AND t.revoked_at IS NULL AND t.expires_at > now()
            ORDER BY t.created_at ASC
            "#,
            user_id,
        )
        .fetch_all(db)
        .await?;
        Ok(rows)
    }

    /// Basic profile fields for userinfo's `profile` scope.
    pub async fn get_user_basic(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Option<(String, Option<String>)>> {
        let row = sqlx::query!(
            "SELECT username, profile_picture_url FROM users WHERE id = $1",
            user_id,
        )
        .fetch_optional(db)
        .await?;
        Ok(row.map(|r| (r.username, r.profile_picture_url)))
    }

    /// All verified email addresses (oldest first), for userinfo's `email`
    /// scope. The first entry is treated as primary.
    pub async fn get_verified_emails(
        &self,
        db: impl PgExecutor<'_>,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query_scalar!(
            r#"
            SELECT email FROM user_emails
            WHERE user_id = $1 AND verified = true
            ORDER BY created_at ASC
            "#,
            user_id,
        )
        .fetch_all(db)
        .await?;
        Ok(rows)
    }

    /// Delete an app scoped to its org. Returns true if a row was removed.
    /// Authorization codes / tokens cascade via FK.
    pub async fn delete_oauth_app(
        &self,
        db: impl PgExecutor<'_>,
        organisation_id: Uuid,
        app_id: Uuid,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query!(
            "DELETE FROM oauth_apps WHERE organisation_id = $1 AND id = $2",
            organisation_id,
            app_id,
        )
        .execute(db)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

// ─── State trait ─────────────────────────────────────────────────────

pub trait OAuthAppRepositoryState {
    fn oauth_app_repository(&self) -> OAuthAppRepository;
}

impl OAuthAppRepositoryState for State {
    fn oauth_app_repository(&self) -> OAuthAppRepository {
        OAuthAppRepository {
            db: self.db.clone(),
        }
    }
}
