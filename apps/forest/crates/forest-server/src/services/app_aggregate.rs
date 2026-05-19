use anyhow::Context;
use forest_event_store::EventStore;
use sha2::Digest;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domains::app::{self, AppAggregate, CreateAppParams};
use crate::services::apps::{AppInfo, AppTokenInfo, CreatedAppToken};

// ============================================================
// Service — orchestrates aggregate + projection for writes
// ============================================================

#[derive(Clone)]
pub struct AppAggregateService {
    event_store: EventStore,
    db: PgPool,
}

impl AppAggregateService {
    pub fn new(event_store: EventStore, db: PgPool) -> Self {
        Self { event_store, db }
    }

    // ----------------------------------------------------------
    // Commands
    // ----------------------------------------------------------

    pub async fn create_app(
        &self,
        organisation_id: Uuid,
        name: &str,
        description: Option<&str>,
        permissions: &serde_json::Value,
        created_by: Uuid,
    ) -> anyhow::Result<AppInfo> {
        let key = app::stream_key(&organisation_id, name);
        let mut root = self
            .event_store
            .load_or_default::<AppAggregate>(&key)
            .await?;

        let app_id = AppAggregate::create(
            &mut root,
            CreateAppParams {
                organisation_id,
                name: name.to_string(),
                description: description.map(String::from),
                permissions: permissions.clone(),
                created_by,
            },
        )?;

        let name_owned = name.to_string();
        let desc_owned = description.map(String::from);
        let perms = permissions.clone();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO apps (id, organisation_id, name, description, permissions, created_by)
                         VALUES ($1, $2, $3, $4, $5, $6)",
                    )
                    .bind(app_id)
                    .bind(organisation_id)
                    .bind(&name_owned)
                    .bind(&desc_owned)
                    .bind(&perms)
                    .bind(created_by)
                    .execute(&mut **tx)
                    .await
                    .context("insert app projection")?;
                    Ok(())
                })
            })
            .await?;

        self.get_app(app_id)
            .await?
            .context("app projection not found after create")
    }

    pub async fn delete_app(&self, app_id: Uuid) -> anyhow::Result<()> {
        let app = self.get_app(app_id).await?.context("app not found")?;
        let key = app::stream_key(&app.organisation_id, &app.name);
        let mut root = self
            .event_store
            .load_or_default::<AppAggregate>(&key)
            .await?;

        AppAggregate::delete(&mut root)?;

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query("DELETE FROM apps WHERE id = $1")
                        .bind(app_id)
                        .execute(&mut **tx)
                        .await
                        .context("delete app projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    pub async fn suspend_app(&self, app_id: Uuid, suspended: bool) -> anyhow::Result<()> {
        let app = self.get_app(app_id).await?.context("app not found")?;
        let key = app::stream_key(&app.organisation_id, &app.name);
        let mut root = self
            .event_store
            .load_or_default::<AppAggregate>(&key)
            .await?;

        if suspended {
            AppAggregate::suspend(&mut root)?;
        } else {
            AppAggregate::unsuspend(&mut root)?;
        }

        if !root.has_pending() {
            return Ok(()); // idempotent, no change
        }

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query("UPDATE apps SET suspended = $2, updated_at = now() WHERE id = $1")
                        .bind(app_id)
                        .bind(suspended)
                        .execute(&mut **tx)
                        .await
                        .context("update app suspension projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    pub async fn create_token(
        &self,
        app_id: Uuid,
        name: &str,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> anyhow::Result<CreatedAppToken> {
        let app = self.get_app(app_id).await?.context("app not found")?;
        let key = app::stream_key(&app.organisation_id, &app.name);
        let mut root = self
            .event_store
            .load_or_default::<AppAggregate>(&key)
            .await?;

        let token_id = AppAggregate::create_token(
            &mut root,
            name.to_string(),
            expires_at,
        )?;

        // Generate raw token and hash — hash goes in projection, raw is returned once
        let mut raw_bytes = [0u8; 32];
        rand::fill(&mut raw_bytes[..]);
        let raw_token = hex::encode(raw_bytes);
        let token_hash = sha2::Sha256::digest(raw_token.as_bytes()).to_vec();

        let name_owned = name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO app_tokens (id, app_id, name, token_hash, expires_at)
                         VALUES ($1, $2, $3, $4, $5)",
                    )
                    .bind(token_id)
                    .bind(app_id)
                    .bind(&name_owned)
                    .bind(&token_hash)
                    .bind(expires_at)
                    .execute(&mut **tx)
                    .await
                    .context("insert app token projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(CreatedAppToken {
            token_id,
            raw_token,
            name: name.to_string(),
            expires_at,
            created_at: chrono::Utc::now(),
        })
    }

    pub async fn revoke_token(&self, token_id: Uuid) -> anyhow::Result<()> {
        // Look up app_id from the token
        let app_id: Uuid = sqlx::query_scalar!(
            "SELECT app_id FROM app_tokens WHERE id = $1",
            token_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup token")?
        .context("token not found")?;

        let app = self.get_app(app_id).await?.context("app not found")?;
        let key = app::stream_key(&app.organisation_id, &app.name);
        let mut root = self
            .event_store
            .load_or_default::<AppAggregate>(&key)
            .await?;

        AppAggregate::revoke_token(&mut root, token_id)?;

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query("UPDATE app_tokens SET revoked = true WHERE id = $1")
                        .bind(token_id)
                        .execute(&mut **tx)
                        .await
                        .context("revoke app token projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    // ----------------------------------------------------------
    // Queries (read from projections — kept here for convenience)
    // ----------------------------------------------------------

    pub async fn get_app(&self, app_id: Uuid) -> anyhow::Result<Option<AppInfo>> {
        let rec = sqlx::query!(
            "SELECT id, organisation_id, name, description, permissions, suspended, created_at, updated_at
             FROM apps WHERE id = $1",
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
            "SELECT id, organisation_id, name, description, permissions, suspended, created_at, updated_at
             FROM apps WHERE organisation_id = $1 ORDER BY created_at DESC",
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

    pub async fn list_tokens(&self, app_id: Uuid) -> anyhow::Result<Vec<AppTokenInfo>> {
        let recs = sqlx::query!(
            "SELECT id, name, expires_at, last_used, revoked, created_at
             FROM app_tokens WHERE app_id = $1 ORDER BY created_at DESC",
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
}

// ============================================================
// State integration
// ============================================================

pub trait AppAggregateServiceState {
    fn app_aggregate_service(&self) -> AppAggregateService;
}

impl AppAggregateServiceState for crate::state::State {
    fn app_aggregate_service(&self) -> AppAggregateService {
        AppAggregateService::new(self.event_store.clone(), self.db.clone())
    }
}
