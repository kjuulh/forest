use crate::event_store::EventStoreExt;
use anyhow::Context;
use chrono::{Duration, Utc};
use mire::EventStore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domains::device_login::{
    self, DeviceGrantAggregate, InitiateDeviceGrantParams, PollStatus,
};

const DEFAULT_EXPIRES_IN_SECONDS: i64 = 900;
const DEFAULT_INTERVAL_SECONDS: i32 = 5;

const MAX_CLIENT_NAME_LEN: usize = 255;
const MAX_CLIENT_VERSION_LEN: usize = 64;
const MAX_SCOPES: usize = 16;
const MAX_SCOPE_LEN: usize = 64;
const MAX_USER_CODE_INPUT_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct InitiatedDeviceGrant {
    pub grant_id: Uuid,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in_seconds: i64,
    pub interval_seconds: i32,
}

#[derive(Debug, Clone)]
pub enum DeviceLoginPollOutcome {
    Pending,
    Approved { user_id: Uuid, scopes: Vec<String> },
    Denied,
    Expired,
    SlowDown,
}

#[derive(Debug, Clone)]
pub struct PendingDeviceGrant {
    pub app_id: Option<Uuid>,
    pub scopes: Vec<String>,
}

#[derive(Clone)]
pub struct DeviceLoginAggregateService {
    event_store: EventStore,
    db: PgPool,
    web_app_url: Option<String>,
}

impl DeviceLoginAggregateService {
    pub fn new(event_store: EventStore, db: PgPool, web_app_url: Option<String>) -> Self {
        Self {
            event_store,
            db,
            web_app_url,
        }
    }

    pub async fn initiate(
        &self,
        client_name: &str,
        client_version: &str,
        scopes: Vec<String>,
        app_id: Option<Uuid>,
    ) -> anyhow::Result<InitiatedDeviceGrant> {
        if client_name.len() > MAX_CLIENT_NAME_LEN {
            anyhow::bail!("client_name too long");
        }
        if client_version.len() > MAX_CLIENT_VERSION_LEN {
            anyhow::bail!("client_version too long");
        }
        if scopes.len() > MAX_SCOPES {
            anyhow::bail!("too many scopes");
        }
        for scope in &scopes {
            if scope.len() > MAX_SCOPE_LEN {
                anyhow::bail!("scope too long");
            }
        }

        let web_app_url = self
            .web_app_url
            .as_deref()
            .context("FOREST_WEB_APP_URL is not configured; web login is disabled")?;

        let now = Utc::now();
        let expires_at = now + Duration::seconds(DEFAULT_EXPIRES_IN_SECONDS);

        let mut candidates = Vec::with_capacity(5);
        {
            let mut rng = rand::rng();
            for _ in 0..5 {
                let device_code = device_login::generate_device_code(&mut rng);
                let device_code_hash = device_login::hash_device_code(&device_code);
                let user_code = device_login::generate_user_code(&mut rng);
                let user_code_normalized = device_login::normalize_user_code(&user_code);
                candidates.push((device_code, device_code_hash, user_code_normalized));
            }
        }

        let mut last_err: Option<anyhow::Error> = None;
        for (device_code, device_code_hash, user_code_normalized) in candidates {
            let stream_key = device_code_hash.clone();
            let mut root = self
                .event_store
                .load_or_default::<DeviceGrantAggregate>(&stream_key)
                .await?;

            let grant_id = match DeviceGrantAggregate::initiate(
                &mut root,
                InitiateDeviceGrantParams {
                    device_code_hash: device_code_hash.clone(),
                    user_code: user_code_normalized.clone(),
                    client_name: client_name.into(),
                    client_version: client_version.into(),
                    scopes: scopes.clone(),
                    expires_at,
                    interval_seconds: DEFAULT_INTERVAL_SECONDS,
                    app_id,
                },
            ) {
                Ok(id) => id,
                Err(e) => {
                    return Err(e);
                }
            };

            let user_code_for_insert = user_code_normalized.clone();
            let client_name_owned = client_name.to_string();
            let client_version_owned = client_version.to_string();
            let scopes_json = serde_json::to_value(&scopes).context("serialize scopes")?;
            let device_code_hash_for_insert = device_code_hash.clone();

            let result = self
                .event_store
                .save_with(&mut root, move |tx| {
                    Box::pin(async move {
                        sqlx::query(
                            "INSERT INTO device_login_grants
                                (id, device_code_hash, user_code, client_name,
                                 client_version, scopes, status, expires_at,
                                 interval_seconds, app_id)
                             VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, $9)",
                        )
                        .bind(grant_id)
                        .bind(&device_code_hash_for_insert)
                        .bind(&user_code_for_insert)
                        .bind(&client_name_owned)
                        .bind(&client_version_owned)
                        .bind(&scopes_json)
                        .bind(expires_at)
                        .bind(DEFAULT_INTERVAL_SECONDS)
                        .bind(app_id)
                        .execute(&mut **tx)
                        .await
                        .context("insert device_login_grants projection")?;
                        Ok(())
                    })
                })
                .await;

            match result {
                Ok(()) => {
                    return Ok(InitiatedDeviceGrant {
                        grant_id,
                        device_code,
                        user_code: format_user_code(&user_code_normalized),
                        verification_uri: format!("{web_app_url}/device"),
                        verification_uri_complete: format!(
                            "{web_app_url}/device?user_code={user_code_normalized}"
                        ),
                        expires_in_seconds: DEFAULT_EXPIRES_IN_SECONDS,
                        interval_seconds: DEFAULT_INTERVAL_SECONDS,
                    });
                }
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("device login: failed to allocate unique code")))
    }

    pub async fn poll(
        &self,
        device_code: &str,
        app: Option<Uuid>,
    ) -> anyhow::Result<DeviceLoginPollOutcome> {
        if device_code.len() > 128 {
            return Ok(DeviceLoginPollOutcome::Expired);
        }
        if device_code.is_empty() {
            return Ok(DeviceLoginPollOutcome::Expired);
        }
        let now = Utc::now();
        let device_code_hash = device_login::hash_device_code(device_code);

        let mut root = self
            .event_store
            .load_or_default::<DeviceGrantAggregate>(&device_code_hash)
            .await?;

        if root.state.app_id != app {
            return Ok(DeviceLoginPollOutcome::Expired);
        }

        let external = root.state.poll_status(now);

        match external {
            PollStatus::Pending => {
                let row = sqlx::query!(
                    r#"SELECT interval_seconds, last_polled_at
                       FROM device_login_grants
                       WHERE device_code_hash = $1"#,
                    device_code_hash,
                )
                .fetch_optional(&self.db)
                .await
                .context("read device_login_grants for slowdown check")?;

                if let Some(row) = row {
                    if let Some(last) = row.last_polled_at {
                        let min_gap = Duration::seconds(row.interval_seconds as i64);
                        if now - last < min_gap {
                            return Ok(DeviceLoginPollOutcome::SlowDown);
                        }
                    }
                }

                sqlx::query!(
                    r#"UPDATE device_login_grants
                       SET last_polled_at = $2
                       WHERE device_code_hash = $1"#,
                    device_code_hash,
                    now,
                )
                .execute(&self.db)
                .await
                .context("update last_polled_at")?;
                Ok(DeviceLoginPollOutcome::Pending)
            }
            PollStatus::Denied => Ok(DeviceLoginPollOutcome::Denied),
            PollStatus::Expired => Ok(DeviceLoginPollOutcome::Expired),
            PollStatus::Approved => {
                let user_id = root
                    .state
                    .approved_user_id
                    .context("approved grant missing user_id")?;
                let scopes = root.state.scopes.clone();

                DeviceGrantAggregate::consume(&mut root, now)?;

                let device_code_hash_for_update = device_code_hash.clone();
                self.event_store
                    .save_with(&mut root, move |tx| {
                        Box::pin(async move {
                            sqlx::query(
                                r#"UPDATE device_login_grants
                                   SET status = 'consumed',
                                       consumed_at = $2,
                                       last_polled_at = $2
                                   WHERE device_code_hash = $1"#,
                            )
                            .bind(&device_code_hash_for_update)
                            .bind(now)
                            .execute(&mut **tx)
                            .await
                            .context("update device_login_grants on consume")?;
                            Ok(())
                        })
                    })
                    .await?;

                Ok(DeviceLoginPollOutcome::Approved { user_id, scopes })
            }
        }
    }

    pub async fn pending(&self, user_code: &str) -> anyhow::Result<Option<PendingDeviceGrant>> {
        if user_code.len() > MAX_USER_CODE_INPUT_LEN {
            return Ok(None);
        }
        let normalized = device_login::normalize_user_code(user_code);
        let row = sqlx::query!(
            r#"SELECT app_id, scopes
               FROM device_login_grants
               WHERE user_code = $1 AND status = 'pending' AND expires_at > now()"#,
            normalized,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup pending device_login_grants by user_code")?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(PendingDeviceGrant {
            app_id: row.app_id,
            scopes: serde_json::from_value(row.scopes).context("device grant scopes")?,
        }))
    }

    pub async fn approve(
        &self,
        user_code: &str,
        user_id: Uuid,
        approving_ip: &str,
        approving_user_agent: &str,
        app: Option<Uuid>,
    ) -> anyhow::Result<()> {
        if user_code.len() > MAX_USER_CODE_INPUT_LEN {
            anyhow::bail!("user_code too long");
        }
        let normalized = device_login::normalize_user_code(user_code);
        let now = Utc::now();

        let row = sqlx::query!(
            r#"SELECT device_code_hash, status, expires_at, app_id
               FROM device_login_grants
               WHERE user_code = $1"#,
            normalized,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup device_login_grants by user_code")?
        .filter(|r| r.app_id == app);

        let row = match row {
            Some(r) => r,
            None => {
                let prefix: String = normalized.chars().take(2).collect();
                tracing::warn!(
                    approving_user_id = %user_id,
                    user_code_prefix = %prefix,
                    "device_login: approve called with unknown user_code"
                );
                anyhow::bail!("no device grant matching that code");
            }
        };

        if row.status != "pending" {
            anyhow::bail!("device grant is not pending (status={})", row.status);
        }
        if now >= row.expires_at {
            anyhow::bail!("device grant has expired");
        }

        let device_code_hash = row.device_code_hash;
        let mut root = self
            .event_store
            .load_or_default::<DeviceGrantAggregate>(&device_code_hash)
            .await?;
        if root.state.app_id != app {
            anyhow::bail!("no device grant matching that code");
        }

        let approving_ip_clean: String = approving_ip.chars().take(45).collect();
        let approving_ua_clean: String = approving_user_agent.chars().take(255).collect();

        DeviceGrantAggregate::approve(
            &mut root,
            user_id,
            approving_ip_clean.clone(),
            approving_ua_clean.clone(),
            now,
        )?;

        let user_code_prefix: String = normalized.chars().take(2).collect();
        tracing::info!(
            approving_user_id = %user_id,
            approving_ip = %approving_ip_clean,
            user_code_prefix = %user_code_prefix,
            client_name = %root.state.client_name,
            client_version = %root.state.client_version,
            "device_login: grant approved"
        );

        let device_code_hash_for_update = device_code_hash.clone();
        let approving_ip_owned = approving_ip_clean;
        let approving_ua_owned = approving_ua_clean;
        self.event_store
            .save_with(&mut root, move |tx| {
                Box::pin(async move {
                    sqlx::query(
                        r#"UPDATE device_login_grants
                           SET status = 'approved',
                               approved_user_id = $2,
                               approving_ip = $3,
                               approving_user_agent = $4,
                               approved_at = $5
                           WHERE device_code_hash = $1"#,
                    )
                    .bind(&device_code_hash_for_update)
                    .bind(user_id)
                    .bind(&approving_ip_owned)
                    .bind(&approving_ua_owned)
                    .bind(now)
                    .execute(&mut **tx)
                    .await
                    .context("update device_login_grants on approve")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    pub async fn deny(
        &self,
        user_code: &str,
        user_id: Uuid,
        app: Option<Uuid>,
    ) -> anyhow::Result<()> {
        if user_code.len() > MAX_USER_CODE_INPUT_LEN {
            anyhow::bail!("user_code too long");
        }
        let normalized = device_login::normalize_user_code(user_code);
        let now = Utc::now();

        let row = sqlx::query!(
            r#"SELECT device_code_hash, status, expires_at, app_id
               FROM device_login_grants
               WHERE user_code = $1"#,
            normalized,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup device_login_grants by user_code")?
        .filter(|r| r.app_id == app);

        let row = match row {
            Some(r) => r,
            None => {
                let prefix: String = normalized.chars().take(2).collect();
                tracing::warn!(
                    denying_user_id = %user_id,
                    user_code_prefix = %prefix,
                    "device_login: deny called with unknown user_code"
                );
                anyhow::bail!("no device grant matching that code");
            }
        };

        if row.status != "pending" {
            anyhow::bail!("device grant is not pending (status={})", row.status);
        }
        if now >= row.expires_at {
            anyhow::bail!("device grant has expired");
        }

        let device_code_hash = row.device_code_hash;
        let mut root = self
            .event_store
            .load_or_default::<DeviceGrantAggregate>(&device_code_hash)
            .await?;
        if root.state.app_id != app {
            anyhow::bail!("no device grant matching that code");
        }

        DeviceGrantAggregate::deny(&mut root, user_id, now)?;

        let user_code_prefix: String = normalized.chars().take(2).collect();
        tracing::info!(
            denying_user_id = %user_id,
            user_code_prefix = %user_code_prefix,
            client_name = %root.state.client_name,
            client_version = %root.state.client_version,
            "device_login: grant denied"
        );

        let device_code_hash_for_update = device_code_hash.clone();
        self.event_store
            .save_with(&mut root, move |tx| {
                Box::pin(async move {
                    sqlx::query(
                        r#"UPDATE device_login_grants
                           SET status = 'denied',
                               approved_user_id = $2
                           WHERE device_code_hash = $1"#,
                    )
                    .bind(&device_code_hash_for_update)
                    .bind(user_id)
                    .execute(&mut **tx)
                    .await
                    .context("update device_login_grants on deny")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }
}

fn format_user_code(normalized: &str) -> String {
    if normalized.len() == 8 {
        format!("{}-{}", &normalized[..4], &normalized[4..])
    } else {
        normalized.to_string()
    }
}

pub trait DeviceLoginAggregateServiceState {
    fn device_login_aggregate_service(&self) -> DeviceLoginAggregateService;
}

impl DeviceLoginAggregateServiceState for crate::state::State {
    fn device_login_aggregate_service(&self) -> DeviceLoginAggregateService {
        DeviceLoginAggregateService::new(
            self.event_store.clone(),
            self.db.clone(),
            self.config.web_app_url.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_user_code_groups_with_dash() {
        assert_eq!(format_user_code("ABCDEFGH"), "ABCD-EFGH");
    }

    #[test]
    fn format_user_code_passthrough_when_wrong_length() {
        assert_eq!(format_user_code("ABC"), "ABC");
    }
}
