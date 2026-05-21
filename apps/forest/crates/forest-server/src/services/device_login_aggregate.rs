//! Service layer for the device-login aggregate.
//!
//! Orchestrates the pure `DeviceGrantAggregate` (in `domains::device_login`)
//! with the projection table `device_login_grants` and the surrounding
//! effectful shell (RNG, clock, DB). See `apps/forest/TASKS/022-device-login.md`.
//!
//! Single-use enforcement: a successful `poll` marks the grant as
//! `Consumed` atomically with the event store write. A second poll of
//! the same `device_code` will see `Consumed` and report `Expired` to
//! the caller — externally indistinguishable from a natural TTL expiry,
//! so a replay attacker cannot learn that the code was already redeemed.

use anyhow::Context;
use chrono::{Duration, Utc};
use forest_event_store::EventStore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domains::device_login::{
    self, DeviceGrantAggregate, InitiateDeviceGrantParams, PollStatus,
};

const DEFAULT_EXPIRES_IN_SECONDS: i64 = 900; // 15 min
const DEFAULT_INTERVAL_SECONDS: i32 = 5;

/// Hard bounds on Initiate inputs. Inputs above these get rejected with
/// `invalid_argument` rather than being silently truncated — clients
/// should fit comfortably, and oversize payloads suggest abuse.
const MAX_CLIENT_NAME_LEN: usize = 255;
const MAX_CLIENT_VERSION_LEN: usize = 64;
const MAX_SCOPES: usize = 16;
const MAX_SCOPE_LEN: usize = 64;
/// Hard upper bound on a submitted user_code (before normalisation).
/// The legit form is "XXXX-XXXX" = 9 chars; we allow some slack for
/// whitespace pasted from terminals but reject anything that looks like
/// an attempt to overflow a column or LIKE-clause.
const MAX_USER_CODE_INPUT_LEN: usize = 32;

/// What a fresh `initiate` returns to the caller. The raw `device_code`
/// only ever lives in memory and in the gRPC response.
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

/// Outcome of a `poll` from the CLI's perspective. The handler maps this
/// to the proto `PollDeviceLoginResponse`.
#[derive(Debug, Clone)]
pub enum DeviceLoginPollOutcome {
    Pending,
    /// The grant was approved and is now consumed. The handler should
    /// issue tokens for `user_id` and return them to the CLI.
    Approved { user_id: Uuid },
    Denied,
    Expired,
    /// The CLI polled faster than `interval_seconds`. RFC 8628 § 3.5.
    SlowDown,
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
    ) -> anyhow::Result<InitiatedDeviceGrant> {
        // Bound caller-controlled inputs before they hit the DB. Each
        // limit is generous for legitimate clients (the canonical
        // client_name "forest-cli/0.3.2 darwin-arm64" is ~36 chars).
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

        // Pre-generate code candidates up front so the (non-Send)
        // ThreadRng never spans an await boundary. Five attempts is
        // generous — collisions on `device_code_hash` (256-bit RNG output)
        // are vanishingly unlikely; the practical cap is `user_code`
        // collisions in the projection unique index.
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
            // Use the device_code_hash as the stream key — it's the
            // identifier we look the grant up by during poll, and it's
            // globally unique (256 bits of entropy).
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
                },
            ) {
                Ok(id) => id,
                Err(e) => {
                    // Should be unreachable — the stream is fresh per
                    // device_code_hash. If it ever happens, propagate.
                    return Err(e);
                }
            };

            let user_code_for_insert = user_code_normalized.clone();
            let client_name_owned = client_name.to_string();
            let client_version_owned = client_version.to_string();
            let scopes_json =
                serde_json::to_value(&scopes).context("serialize scopes")?;
            let device_code_hash_for_insert = device_code_hash.clone();

            let result = self
                .event_store
                .save_with(&mut root, move |_events, tx| {
                    Box::pin(async move {
                        sqlx::query(
                            "INSERT INTO device_login_grants
                                (id, device_code_hash, user_code, client_name,
                                 client_version, scopes, status, expires_at,
                                 interval_seconds)
                             VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8)",
                        )
                        .bind(grant_id)
                        .bind(&device_code_hash_for_insert)
                        .bind(&user_code_for_insert)
                        .bind(&client_name_owned)
                        .bind(&client_version_owned)
                        .bind(&scopes_json)
                        .bind(expires_at)
                        .bind(DEFAULT_INTERVAL_SECONDS)
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
                    // The most common cause is a `user_code` index
                    // collision — retry with fresh codes.
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("device login: failed to allocate unique code")))
    }

    /// Poll a device grant. If approved within TTL, atomically marks it
    /// consumed and returns the `user_id` so the handler can issue tokens.
    pub async fn poll(&self, device_code: &str) -> anyhow::Result<DeviceLoginPollOutcome> {
        // Bound the input length so an attacker can't ship megabytes
        // hoping to slow the hasher. The legitimate device_code is 43
        // ASCII chars; we accept up to 128 to leave headroom for future
        // changes.
        if device_code.len() > 128 {
            return Ok(DeviceLoginPollOutcome::Expired);
        }
        // Unknown / malformed codes are masked as Expired (same as the
        // anti-enumeration treatment of an unknown but well-formed code).
        if device_code.is_empty() {
            return Ok(DeviceLoginPollOutcome::Expired);
        }
        let now = Utc::now();
        let device_code_hash = device_login::hash_device_code(device_code);

        let mut root = self
            .event_store
            .load_or_default::<DeviceGrantAggregate>(&device_code_hash)
            .await?;

        let external = root.state.poll_status(now);

        // Slow-down only applies to *pending* grants. Terminal states
        // (denied / expired / consumed-masked-as-expired) must be
        // returned regardless of polling cadence — otherwise a replay
        // attacker can distinguish "code was redeemed" from "code never
        // existed" by triggering a SlowDown only on known codes.
        match external {
            PollStatus::Pending => {
                // Read the projection to enforce interval_seconds.
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
                // Atomically consume + update projection.
                let user_id = root
                    .state
                    .approved_user_id
                    .context("approved grant missing user_id")?;

                DeviceGrantAggregate::consume(&mut root, now)?;

                let device_code_hash_for_update = device_code_hash.clone();
                self.event_store
                    .save_with(&mut root, move |_events, tx| {
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

                Ok(DeviceLoginPollOutcome::Approved { user_id })
            }
        }
    }

    pub async fn approve(
        &self,
        user_code: &str,
        user_id: Uuid,
        approving_ip: &str,
        approving_user_agent: &str,
    ) -> anyhow::Result<()> {
        if user_code.len() > MAX_USER_CODE_INPUT_LEN {
            anyhow::bail!("user_code too long");
        }
        let normalized = device_login::normalize_user_code(user_code);
        let now = Utc::now();

        // Resolve user_code → device_code_hash via the projection.
        let row = sqlx::query!(
            r#"SELECT device_code_hash, status, expires_at
               FROM device_login_grants
               WHERE user_code = $1"#,
            normalized,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup device_login_grants by user_code")?;

        let row = match row {
            Some(r) => r,
            None => {
                // Audit the failed lookup. Do NOT echo the submitted
                // user_code to logs — log only a short prefix so brute
                // forcers can't seed a search of operator-side logs.
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

        // Truncate IP/UA before persistence — they're caller-supplied
        // (forage forwards them on the user's behalf) so we treat them
        // as untrusted strings even though the calling forage backend is
        // trusted. Length caps prevent log/row bloat.
        let approving_ip_clean: String = approving_ip.chars().take(45).collect();
        let approving_ua_clean: String = approving_user_agent.chars().take(255).collect();

        DeviceGrantAggregate::approve(
            &mut root,
            user_id,
            approving_ip_clean.clone(),
            approving_ua_clean.clone(),
            now,
        )?;

        // Audit log — captures who approved, from where, what code prefix
        // matched. The full user_code and the device_code never appear.
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
            .save_with(&mut root, move |_events, tx| {
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

    pub async fn deny(&self, user_code: &str, user_id: Uuid) -> anyhow::Result<()> {
        if user_code.len() > MAX_USER_CODE_INPUT_LEN {
            anyhow::bail!("user_code too long");
        }
        let normalized = device_login::normalize_user_code(user_code);
        let now = Utc::now();

        let row = sqlx::query!(
            r#"SELECT device_code_hash, status, expires_at
               FROM device_login_grants
               WHERE user_code = $1"#,
            normalized,
        )
        .fetch_optional(&self.db)
        .await
        .context("lookup device_login_grants by user_code")?;

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
            .save_with(&mut root, move |_events, tx| {
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

/// Format a normalized 8-char user code as `XXXX-XXXX`. Inverse of
/// `normalize_user_code` for the display side.
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
