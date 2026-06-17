use std::time::Duration;

use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::{State, repositories::oauth_apps::OAuthAppRepository};

/// Periodically prunes dead OAuth rows (expired/consumed authorization codes,
/// fully-expired or long-revoked access tokens). Pure housekeeping — these
/// rows never resolve, so this only bounds table growth.
pub struct OAuthReaper {
    repo: OAuthAppRepository,
    interval: Duration,
}

impl OAuthReaper {
    pub fn new(state: &State) -> Self {
        Self {
            repo: OAuthAppRepository::new(state.db.clone()),
            // OAuth tables churn slowly; hourly is plenty.
            interval: Duration::from_secs(60 * 60),
        }
    }

    async fn reap(&self) -> anyhow::Result<()> {
        let (codes, tokens) = self.repo.reap_expired(self.repo.pool()).await?;
        if codes > 0 || tokens > 0 {
            tracing::info!(
                reaped_codes = codes,
                reaped_tokens = tokens,
                "oauth reaper pruned dead rows"
            );
        }
        Ok(())
    }
}

impl Component for OAuthReaper {
    fn info(&self) -> ComponentInfo {
        "forest-server/oauth-reaper".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(e) = self.reap().await {
                        tracing::error!("oauth reaper error: {e:#}");
                    }
                }
            }
        }

        Ok(())
    }
}
