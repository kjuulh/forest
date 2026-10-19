use std::time::Duration;

use anyhow::Context;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ReleaseTokenScope {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub artifact_id: Uuid,
    pub destination_id: Uuid,
    pub project_id: Uuid,
    pub environment: String,
    pub runner_id: String,
}

#[derive(Clone)]
pub struct ReleaseTokenRegistry {
    db: PgPool,
}

impl ReleaseTokenRegistry {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn create_token(
        &self,
        scope: ReleaseTokenScope,
        ttl: Duration,
    ) -> anyhow::Result<String> {
        let raw: [u8; 32] = rand::random();
        let token_hex = hex::encode(raw);
        let token_hash = Sha256::digest(raw).to_vec();

        let expires = chrono::Utc::now()
            + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::hours(1));

        sqlx::query!(
            "
            INSERT INTO release_tokens (
                token_hash, release_id, release_intent_id, artifact_id,
                destination_id, project_id, runner_id, environment, expires
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ",
            &token_hash,
            scope.release_id,
            scope.release_intent_id,
            scope.artifact_id,
            scope.destination_id,
            scope.project_id,
            scope.runner_id,
            scope.environment,
            expires,
        )
        .execute(&self.db)
        .await
        .context("failed to insert release token")?;

        Ok(token_hex)
    }

    pub async fn validate_token(
        &self,
        token_hex: &str,
    ) -> anyhow::Result<Option<ReleaseTokenScope>> {
        let raw = hex::decode(token_hex).context("invalid token format: not valid hex")?;
        let token_hash = Sha256::digest(&raw).to_vec();

        let record = sqlx::query!(
            "
            SELECT release_id, release_intent_id, artifact_id, destination_id,
                   project_id, runner_id, environment
            FROM release_tokens
            WHERE token_hash = $1
              AND NOT revoked
              AND expires > now()
            ",
            &token_hash,
        )
        .fetch_optional(&self.db)
        .await
        .context("failed to validate release token")?;

        Ok(record.map(|r| ReleaseTokenScope {
            release_id: r.release_id,
            release_intent_id: r.release_intent_id,
            artifact_id: r.artifact_id,
            destination_id: r.destination_id,
            project_id: r.project_id,
            environment: r.environment,
            runner_id: r.runner_id,
        }))
    }

    pub async fn revoke_token(&self, token_hex: &str) -> anyhow::Result<()> {
        let raw = hex::decode(token_hex).context("invalid token format")?;
        let token_hash = Sha256::digest(&raw).to_vec();

        sqlx::query!(
            "UPDATE release_tokens SET revoked = true WHERE token_hash = $1",
            &token_hash,
        )
        .execute(&self.db)
        .await
        .context("failed to revoke release token")?;

        Ok(())
    }

    pub async fn revoke_runner_tokens(
        &self,
        runner_id: &str,
    ) -> anyhow::Result<Vec<ReleaseTokenScope>> {
        let records = sqlx::query!(
            "
            UPDATE release_tokens
            SET revoked = true
            WHERE runner_id = $1
              AND NOT revoked
              AND expires > now()
            RETURNING release_id, release_intent_id, artifact_id,
                      destination_id, project_id, environment, runner_id
            ",
            runner_id,
        )
        .fetch_all(&self.db)
        .await
        .context("failed to revoke runner tokens")?;

        Ok(records
            .into_iter()
            .map(|r| ReleaseTokenScope {
                release_id: r.release_id,
                release_intent_id: r.release_intent_id,
                artifact_id: r.artifact_id,
                destination_id: r.destination_id,
                project_id: r.project_id,
                environment: r.environment,
                runner_id: r.runner_id,
            })
            .collect())
    }

    pub async fn cleanup_expired(&self) -> anyhow::Result<u64> {
        let result =
            sqlx::query!("DELETE FROM release_tokens WHERE expires < now() - interval '1 day'")
                .execute(&self.db)
                .await
                .context("failed to cleanup expired tokens")?;

        Ok(result.rows_affected())
    }
}

pub trait ReleaseTokenRegistryState {
    fn release_token_registry(&self) -> ReleaseTokenRegistry;
}

impl ReleaseTokenRegistryState for crate::State {
    fn release_token_registry(&self) -> ReleaseTokenRegistry {
        ReleaseTokenRegistry::new(self.db.clone())
    }
}
