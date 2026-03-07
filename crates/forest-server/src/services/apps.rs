use sha2::Digest;
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

pub struct AppService {
    db: PgPool,
}

impl AppService {
    pub async fn create_app(
        &self,
        organisation_id: Uuid,
        name: &str,
        description: Option<&str>,
        permissions: &serde_json::Value,
        created_by: Uuid,
    ) -> anyhow::Result<AppInfo> {
        let id = Uuid::now_v7();

        let rec = sqlx::query!(
            r#"
            INSERT INTO apps (id, organisation_id, name, description, permissions, created_by)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, organisation_id, name, description, permissions, suspended, created_at, updated_at
            "#,
            id,
            organisation_id,
            name,
            description,
            permissions,
            created_by,
        )
        .fetch_one(&self.db)
        .await?;

        Ok(AppInfo {
            id: rec.id,
            organisation_id: rec.organisation_id,
            name: rec.name,
            description: rec.description,
            permissions: rec.permissions,
            suspended: rec.suspended,
            created_at: rec.created_at,
        })
    }

    pub async fn get_app(&self, app_id: Uuid) -> anyhow::Result<Option<AppInfo>> {
        let rec = sqlx::query!(
            r#"
            SELECT id, organisation_id, name, description, permissions, suspended, created_at, updated_at
            FROM apps
            WHERE id = $1
            "#,
            app_id,
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(rec.map(|r| AppInfo {
            id: r.id,
            organisation_id: r.organisation_id,
            name: r.name,
            description: r.description,
            permissions: r.permissions,
            suspended: r.suspended,
            created_at: r.created_at,
        }))
    }

    pub async fn list_apps(&self, organisation_id: Uuid) -> anyhow::Result<Vec<AppInfo>> {
        let recs = sqlx::query!(
            r#"
            SELECT id, organisation_id, name, description, permissions, suspended, created_at, updated_at
            FROM apps
            WHERE organisation_id = $1
            ORDER BY created_at DESC
            "#,
            organisation_id,
        )
        .fetch_all(&self.db)
        .await?;

        Ok(recs
            .into_iter()
            .map(|r| AppInfo {
                id: r.id,
                organisation_id: r.organisation_id,
                name: r.name,
                description: r.description,
                permissions: r.permissions,
                suspended: r.suspended,
                created_at: r.created_at,
            })
            .collect())
    }

    pub async fn delete_app(&self, app_id: Uuid) -> anyhow::Result<()> {
        sqlx::query!("DELETE FROM apps WHERE id = $1", app_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn suspend_app(&self, app_id: Uuid, suspended: bool) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE apps SET suspended = $2, updated_at = now() WHERE id = $1",
            app_id,
            suspended,
        )
        .execute(&self.db)
        .await?;
        Ok(())
    }

    // -- Token management ---------------------------------------------------------

    pub async fn create_token(
        &self,
        app_id: Uuid,
        name: &str,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> anyhow::Result<CreatedAppToken> {
        let id = Uuid::now_v7();

        // Generate a random token and store only its hash
        let mut raw = [0u8; 32];
        rand::fill(&mut raw[..]);
        let raw_token = hex::encode(raw);
        let token_hash = sha2::Sha256::digest(raw_token.as_bytes()).to_vec();

        sqlx::query!(
            r#"
            INSERT INTO app_tokens (id, app_id, name, token_hash, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            id,
            app_id,
            name,
            &token_hash,
            expires_at,
        )
        .execute(&self.db)
        .await?;

        Ok(CreatedAppToken {
            token_id: id,
            raw_token,
            name: name.to_string(),
            expires_at,
            created_at: chrono::Utc::now(),
        })
    }

    pub async fn list_tokens(&self, app_id: Uuid) -> anyhow::Result<Vec<AppTokenInfo>> {
        let recs = sqlx::query!(
            r#"
            SELECT id, name, expires_at, last_used, revoked, created_at
            FROM app_tokens
            WHERE app_id = $1
            ORDER BY created_at DESC
            "#,
            app_id,
        )
        .fetch_all(&self.db)
        .await?;

        Ok(recs
            .into_iter()
            .map(|r| AppTokenInfo {
                id: r.id,
                name: r.name,
                expires_at: r.expires_at,
                last_used: r.last_used,
                revoked: r.revoked,
                created_at: r.created_at,
            })
            .collect())
    }

    pub async fn revoke_token(&self, token_id: Uuid) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE app_tokens SET revoked = true WHERE id = $1",
            token_id,
        )
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

// -- Return types -----------------------------------------------------------------

pub struct AppInfo {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub permissions: serde_json::Value,
    pub suspended: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct CreatedAppToken {
    pub token_id: Uuid,
    pub raw_token: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct AppTokenInfo {
    pub id: Uuid,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// -- State trait ------------------------------------------------------------------

pub trait AppServiceState {
    fn app_service(&self) -> AppService;
}

impl AppServiceState for State {
    fn app_service(&self) -> AppService {
        AppService {
            db: self.db.clone(),
        }
    }
}
