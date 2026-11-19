use forage_core::integrations::{
    CreateIntegrationInput, DeliveryStatus, Integration, IntegrationConfig, IntegrationError,
    IntegrationStore, IntegrationType, NotificationDelivery, NotificationRule, SlackMessageRef,
    SlackUserLink, NOTIFICATION_TYPES,
};
use forage_core::integrations::baseline::{DirectoryMatch, SlackBaseline};
use forage_core::integrations::SlackReleaseLock;
use forage_core::integrations::slack_release::SlackEmojiSet;
use sqlx::PgPool;
use uuid::Uuid;

pub struct PgIntegrationStore {
    pool: PgPool,
    encryption_key: Vec<u8>,
}

impl PgIntegrationStore {
    pub fn new(pool: PgPool, encryption_key: Vec<u8>) -> Self {
        Self {
            pool,
            encryption_key,
        }
    }

    fn encrypt_config(&self, config: &IntegrationConfig) -> Result<Vec<u8>, IntegrationError> {
        let json = serde_json::to_vec(config)
            .map_err(|e| IntegrationError::Encryption(e.to_string()))?;
        Ok(xor_bytes(&json, &self.encryption_key))
    }

    fn decrypt_config(&self, encrypted: &[u8]) -> Result<IntegrationConfig, IntegrationError> {
        let json = xor_bytes(encrypted, &self.encryption_key);
        serde_json::from_slice(&json)
            .map_err(|e| IntegrationError::Encryption(format!("decrypt failed: {e}")))
    }

    fn row_to_integration(&self, row: IntegrationRow) -> Result<Integration, IntegrationError> {
        let config = self.decrypt_config(&row.config_encrypted)?;
        let integration_type = IntegrationType::parse(&row.integration_type)
            .ok_or_else(|| IntegrationError::Store(format!("unknown type: {}", row.integration_type)))?;
        Ok(Integration {
            id: row.id.to_string(),
            organisation: row.organisation,
            integration_type,
            name: row.name,
            config,
            enabled: row.enabled,
            created_by: row.created_by,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
            api_token: None,
        })
    }
}

fn xor_bytes(data: &[u8], key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

#[async_trait::async_trait]
impl IntegrationStore for PgIntegrationStore {
    async fn list_integrations(
        &self,
        organisation: &str,
    ) -> Result<Vec<Integration>, IntegrationError> {
        let rows: Vec<IntegrationRow> = sqlx::query_as(
            "SELECT id, organisation, integration_type, name, config_encrypted, enabled, created_by, created_at, updated_at
             FROM integrations WHERE organisation = $1 ORDER BY created_at",
        )
        .bind(organisation)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        rows.into_iter().map(|r| self.row_to_integration(r)).collect()
    }

    async fn get_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<Integration, IntegrationError> {
        let uuid: Uuid = id
            .parse()
            .map_err(|_| IntegrationError::NotFound(id.to_string()))?;

        let row: IntegrationRow = sqlx::query_as(
            "SELECT id, organisation, integration_type, name, config_encrypted, enabled, created_by, created_at, updated_at
             FROM integrations WHERE id = $1 AND organisation = $2",
        )
        .bind(uuid)
        .bind(organisation)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?
        .ok_or_else(|| IntegrationError::NotFound(id.to_string()))?;

        self.row_to_integration(row)
    }

    async fn create_integration(
        &self,
        input: &CreateIntegrationInput,
    ) -> Result<Integration, IntegrationError> {
        use forage_core::integrations::{generate_api_token, hash_api_token};

        let id = Uuid::new_v4();
        let encrypted = self.encrypt_config(&input.config)?;
        let now = chrono::Utc::now();
        let raw_token = generate_api_token();
        let token_hash = hash_api_token(&raw_token);

        sqlx::query(
            "INSERT INTO integrations (id, organisation, integration_type, name, config_encrypted, enabled, created_by, created_at, updated_at, api_token_hash)
             VALUES ($1, $2, $3, $4, $5, true, $6, $7, $7, $8)",
        )
        .bind(id)
        .bind(&input.organisation)
        .bind(input.integration_type.as_str())
        .bind(&input.name)
        .bind(&encrypted)
        .bind(&input.created_by)
        .bind(now)
        .bind(&token_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("duplicate key") || e.to_string().contains("unique") {
                IntegrationError::Duplicate(format!(
                    "Integration '{}' already exists in org '{}'",
                    input.name, input.organisation
                ))
            } else {
                IntegrationError::Store(e.to_string())
            }
        })?;

        for nt in NOTIFICATION_TYPES {
            sqlx::query(
                "INSERT INTO notification_rules (id, integration_id, notification_type, enabled)
                 VALUES ($1, $2, $3, true)",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(*nt)
            .execute(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        }

        Ok(Integration {
            id: id.to_string(),
            organisation: input.organisation.clone(),
            integration_type: input.integration_type,
            name: input.name.clone(),
            config: input.config.clone(),
            enabled: true,
            created_by: input.created_by.clone(),
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            api_token: Some(raw_token),
        })
    }

    async fn set_integration_enabled(
        &self,
        organisation: &str,
        id: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError> {
        let uuid: Uuid = id
            .parse()
            .map_err(|_| IntegrationError::NotFound(id.to_string()))?;

        let result = sqlx::query(
            "UPDATE integrations SET enabled = $1, updated_at = now() WHERE id = $2 AND organisation = $3",
        )
        .bind(enabled)
        .bind(uuid)
        .bind(organisation)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(IntegrationError::NotFound(id.to_string()));
        }
        Ok(())
    }

    async fn delete_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<(), IntegrationError> {
        let uuid: Uuid = id
            .parse()
            .map_err(|_| IntegrationError::NotFound(id.to_string()))?;

        let result = sqlx::query("DELETE FROM integrations WHERE id = $1 AND organisation = $2")
            .bind(uuid)
            .bind(organisation)
            .execute(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(IntegrationError::NotFound(id.to_string()));
        }
        Ok(())
    }

    async fn update_integration_config(
        &self,
        organisation: &str,
        id: &str,
        name: &str,
        config: &IntegrationConfig,
    ) -> Result<(), IntegrationError> {
        let uuid: Uuid = id
            .parse()
            .map_err(|_| IntegrationError::NotFound(id.to_string()))?;
        let encrypted = self.encrypt_config(config)?;

        let result = sqlx::query(
            "UPDATE integrations SET name = $1, config_encrypted = $2, updated_at = NOW()
             WHERE id = $3 AND organisation = $4",
        )
        .bind(name)
        .bind(&encrypted)
        .bind(uuid)
        .bind(organisation)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(IntegrationError::NotFound(id.to_string()));
        }
        Ok(())
    }

    async fn list_rules(
        &self,
        integration_id: &str,
    ) -> Result<Vec<NotificationRule>, IntegrationError> {
        let uuid: Uuid = integration_id
            .parse()
            .map_err(|_| IntegrationError::NotFound(integration_id.to_string()))?;

        let rows: Vec<RuleRow> = sqlx::query_as(
            "SELECT id, integration_id, notification_type, enabled
             FROM notification_rules WHERE integration_id = $1 ORDER BY notification_type",
        )
        .bind(uuid)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| NotificationRule {
                id: r.id.to_string(),
                integration_id: r.integration_id.to_string(),
                notification_type: r.notification_type,
                enabled: r.enabled,
            })
            .collect())
    }

    async fn set_rule_enabled(
        &self,
        integration_id: &str,
        notification_type: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError> {
        let uuid: Uuid = integration_id
            .parse()
            .map_err(|_| IntegrationError::NotFound(integration_id.to_string()))?;

        let result = sqlx::query(
            "UPDATE notification_rules SET enabled = $1
             WHERE integration_id = $2 AND notification_type = $3",
        )
        .bind(enabled)
        .bind(uuid)
        .bind(notification_type)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        if result.rows_affected() == 0 {
            sqlx::query(
                "INSERT INTO notification_rules (id, integration_id, notification_type, enabled)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(Uuid::new_v4())
            .bind(uuid)
            .bind(notification_type)
            .bind(enabled)
            .execute(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        }
        Ok(())
    }

    async fn record_delivery(
        &self,
        integration_id: &str,
        notification_id: &str,
        status: DeliveryStatus,
        error_message: Option<&str>,
    ) -> Result<(), IntegrationError> {
        let uuid: Uuid = integration_id
            .parse()
            .map_err(|_| IntegrationError::NotFound(integration_id.to_string()))?;

        sqlx::query(
            "INSERT INTO notification_deliveries (id, integration_id, notification_id, status, error_message, attempted_at)
             VALUES ($1, $2, $3, $4, $5, now())",
        )
        .bind(Uuid::new_v4())
        .bind(uuid)
        .bind(notification_id)
        .bind(status.as_str())
        .bind(error_message)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        Ok(())
    }

    async fn list_deliveries(
        &self,
        integration_id: &str,
        limit: usize,
    ) -> Result<Vec<NotificationDelivery>, IntegrationError> {
        let uuid: Uuid = integration_id
            .parse()
            .map_err(|_| IntegrationError::NotFound(integration_id.to_string()))?;

        let rows: Vec<DeliveryRow> = sqlx::query_as(
            "SELECT id, integration_id, notification_id, status, error_message, attempted_at
             FROM notification_deliveries
             WHERE integration_id = $1
             ORDER BY attempted_at DESC
             LIMIT $2",
        )
        .bind(uuid)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let status = DeliveryStatus::parse(&r.status).unwrap_or(DeliveryStatus::Pending);
                NotificationDelivery {
                    id: r.id.to_string(),
                    integration_id: r.integration_id.to_string(),
                    notification_id: r.notification_id,
                    status,
                    error_message: r.error_message,
                    attempted_at: r.attempted_at.to_rfc3339(),
                }
            })
            .collect())
    }

    async fn list_matching_integrations(
        &self,
        organisation: &str,
        notification_type: &str,
    ) -> Result<Vec<Integration>, IntegrationError> {
        let rows: Vec<IntegrationRow> = sqlx::query_as(
            "SELECT i.id, i.organisation, i.integration_type, i.name, i.config_encrypted, i.enabled, i.created_by, i.created_at, i.updated_at
             FROM integrations i
             JOIN notification_rules nr ON nr.integration_id = i.id
             WHERE i.organisation = $1
               AND i.enabled = true
               AND nr.notification_type = $2
               AND nr.enabled = true
             ORDER BY i.created_at",
        )
        .bind(organisation)
        .bind(notification_type)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        rows.into_iter().map(|r| self.row_to_integration(r)).collect()
    }

    async fn get_integration_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Integration, IntegrationError> {
        let row: IntegrationRow = sqlx::query_as(
            "SELECT id, organisation, integration_type, name, config_encrypted, enabled, created_by, created_at, updated_at
             FROM integrations WHERE api_token_hash = $1 AND enabled = true",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?
        .ok_or_else(|| IntegrationError::NotFound("invalid token".to_string()))?;

        self.row_to_integration(row)
    }

    async fn get_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Option<SlackUserLink>, IntegrationError> {
        let row: Option<SlackUserLinkRow> = sqlx::query_as(
            "SELECT id, user_id, team_id, team_name, slack_user_id, slack_username, created_at
             FROM slack_user_links WHERE user_id = $1 AND team_id = $2",
        )
        .bind(user_id)
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        Ok(row.map(|r| SlackUserLink {
            id: r.id.to_string(),
            user_id: r.user_id,
            team_id: r.team_id,
            team_name: r.team_name,
            slack_user_id: r.slack_user_id,
            slack_username: r.slack_username,
            created_at: r.created_at.to_rfc3339(),
        }))
    }

    async fn upsert_slack_user_link(
        &self,
        link: &SlackUserLink,
    ) -> Result<(), IntegrationError> {
        sqlx::query(
            "INSERT INTO slack_user_links (id, user_id, team_id, team_name, slack_user_id, slack_username, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, NOW())
             ON CONFLICT (user_id, team_id) DO UPDATE SET
               slack_user_id = EXCLUDED.slack_user_id,
               slack_username = EXCLUDED.slack_username,
               team_name = EXCLUDED.team_name",
        )
        .bind(Uuid::parse_str(&link.id).map_err(|e| IntegrationError::Store(e.to_string()))?)
        .bind(&link.user_id)
        .bind(&link.team_id)
        .bind(&link.team_name)
        .bind(&link.slack_user_id)
        .bind(&link.slack_username)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<(), IntegrationError> {
        sqlx::query("DELETE FROM slack_user_links WHERE user_id = $1 AND team_id = $2")
            .bind(user_id)
            .bind(team_id)
            .execute(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_slack_user_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<SlackUserLink>, IntegrationError> {
        let rows: Vec<SlackUserLinkRow> = sqlx::query_as(
            "SELECT id, user_id, team_id, team_name, slack_user_id, slack_username, created_at
             FROM slack_user_links WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| SlackUserLink {
                id: r.id.to_string(),
                user_id: r.user_id,
                team_id: r.team_id,
                team_name: r.team_name,
                slack_user_id: r.slack_user_id,
                slack_username: r.slack_username,
                created_at: r.created_at.to_rfc3339(),
            })
            .collect())
    }

    async fn get_slack_message_ref(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Option<SlackMessageRef>, IntegrationError> {
        let iid =
            Uuid::parse_str(integration_id).map_err(|e| IntegrationError::Store(e.to_string()))?;
        let row: Option<SlackMessageRefRow> = sqlx::query_as(
            "SELECT id, integration_id, release_id, channel_id, message_ts, last_event_type, destinations, release_title, thread_keys, rendered_hash, destination_count, created_at, updated_at
             FROM slack_message_refs WHERE integration_id = $1 AND release_id = $2",
        )
        .bind(iid)
        .bind(release_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;

        Ok(row.map(|r| SlackMessageRef {
            id: r.id.to_string(),
            integration_id: r.integration_id.to_string(),
            release_id: r.release_id,
            channel_id: r.channel_id,
            message_ts: r.message_ts,
            last_event_type: r.last_event_type,
            destinations: serde_json::from_value(r.destinations).unwrap_or_default(),
            release_title: r.release_title,
            thread_keys: serde_json::from_value(r.thread_keys).unwrap_or_default(),
            rendered_hash: r.rendered_hash,
            destination_count: r.destination_count,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        }))
    }

    async fn upsert_slack_message_ref(
        &self,
        msg_ref: &SlackMessageRef,
    ) -> Result<(), IntegrationError> {
        let iid = Uuid::parse_str(&msg_ref.integration_id)
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        let destinations_json = serde_json::to_value(&msg_ref.destinations)
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        sqlx::query(
            "INSERT INTO slack_message_refs (id, integration_id, release_id, channel_id, message_ts, last_event_type, destinations, release_title, rendered_hash, destination_count, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW(), NOW())
             ON CONFLICT (integration_id, release_id) DO UPDATE SET
               thread_keys = CASE WHEN slack_message_refs.message_ts = EXCLUDED.message_ts
                                  THEN slack_message_refs.thread_keys ELSE '[]'::jsonb END,
               rendered_hash = EXCLUDED.rendered_hash,
               destination_count = GREATEST(slack_message_refs.destination_count, EXCLUDED.destination_count),
               message_ts = EXCLUDED.message_ts,
               last_event_type = EXCLUDED.last_event_type,
               destinations = EXCLUDED.destinations,
               release_title = EXCLUDED.release_title,
               updated_at = NOW()",
        )
        .bind(Uuid::parse_str(&msg_ref.id).map_err(|e| IntegrationError::Store(e.to_string()))?)
        .bind(iid)
        .bind(&msg_ref.release_id)
        .bind(&msg_ref.channel_id)
        .bind(&msg_ref.message_ts)
        .bind(&msg_ref.last_event_type)
        .bind(destinations_json)
        .bind(&msg_ref.release_title)
        .bind(&msg_ref.rendered_hash)
        .bind(msg_ref.destination_count)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn lock_slack_release(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Box<dyn SlackReleaseLock>, IntegrationError> {
        let store = |e: sqlx::Error| IntegrationError::Store(e.to_string());
        let mut tx = self.pool.begin().await.map_err(store)?;
        sqlx::query("SET LOCAL lock_timeout = '120s'")
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("slack-release:{integration_id}:{release_id}"))
            .execute(&mut *tx)
            .await
            .map_err(store)?;
        Ok(Box::new(PgReleaseLock(tx)))
    }

    async fn claim_slack_thread_key(
        &self,
        integration_id: &str,
        release_id: &str,
        key: &str,
    ) -> Result<bool, IntegrationError> {
        let iid =
            Uuid::parse_str(integration_id).map_err(|e| IntegrationError::Store(e.to_string()))?;
        let claimed = sqlx::query(
            "UPDATE slack_message_refs
             SET thread_keys = thread_keys || jsonb_build_array($3::text)
             WHERE integration_id = $1 AND release_id = $2 AND NOT thread_keys ? $3",
        )
        .bind(iid)
        .bind(release_id)
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?
        .rows_affected();
        Ok(claimed == 1)
    }

    async fn release_slack_thread_key(
        &self,
        integration_id: &str,
        release_id: &str,
        key: &str,
    ) -> Result<(), IntegrationError> {
        let iid =
            Uuid::parse_str(integration_id).map_err(|e| IntegrationError::Store(e.to_string()))?;
        sqlx::query(
            "UPDATE slack_message_refs SET thread_keys = thread_keys - $3
             WHERE integration_id = $1 AND release_id = $2",
        )
        .bind(iid)
        .bind(release_id)
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn get_slack_emoji_set(
        &self,
        organisation: &str,
    ) -> Result<Option<SlackEmojiSet>, IntegrationError> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT emoji FROM slack_emoji_sets WHERE organisation = $1")
                .bind(organisation)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(row.map(|(v,)| serde_json::from_value(v).unwrap_or_default()))
    }

    async fn set_slack_emoji_set(
        &self,
        organisation: &str,
        set: &SlackEmojiSet,
        updated_by: &str,
    ) -> Result<(), IntegrationError> {
        let emoji =
            serde_json::to_value(set).map_err(|e| IntegrationError::Store(e.to_string()))?;
        sqlx::query(
            "INSERT INTO slack_emoji_sets (organisation, emoji, updated_by, updated_at)
             VALUES ($1, $2, $3, NOW())
             ON CONFLICT (organisation) DO UPDATE SET
               emoji = EXCLUDED.emoji, updated_by = EXCLUDED.updated_by, updated_at = NOW()",
        )
        .bind(organisation)
        .bind(emoji)
        .bind(updated_by)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn get_slack_baseline(
        &self,
        integration_id: &str,
    ) -> Result<Option<SlackBaseline>, IntegrationError> {
        let Ok(iid) = Uuid::parse_str(integration_id) else {
            return Ok(None);
        };
        let row: Option<(bool, serde_json::Value, String, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as(
                "SELECT directory_mapping, dm_defaults, updated_by, updated_at
                 FROM slack_baselines WHERE integration_id = $1",
            )
            .bind(iid)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(row.map(|(directory_mapping, dm_defaults, updated_by, updated_at)| SlackBaseline {
            integration_id: integration_id.to_string(),
            directory_mapping,
            dm_defaults: serde_json::from_value(dm_defaults).unwrap_or_default(),
            updated_by,
            updated_at: updated_at.to_rfc3339(),
        }))
    }

    async fn upsert_slack_baseline(&self, b: &SlackBaseline) -> Result<(), IntegrationError> {
        let iid = Uuid::parse_str(&b.integration_id)
            .map_err(|_| IntegrationError::NotFound(b.integration_id.clone()))?;
        let defaults =
            serde_json::to_value(&b.dm_defaults).map_err(|e| IntegrationError::Store(e.to_string()))?;
        sqlx::query(
            "INSERT INTO slack_baselines (integration_id, directory_mapping, dm_defaults, updated_by, updated_at)
             VALUES ($1, $2, $3, $4, NOW())
             ON CONFLICT (integration_id) DO UPDATE SET
               directory_mapping = EXCLUDED.directory_mapping,
               dm_defaults = EXCLUDED.dm_defaults,
               updated_by = EXCLUDED.updated_by,
               updated_at = NOW()",
        )
        .bind(iid)
        .bind(b.directory_mapping)
        .bind(defaults)
        .bind(&b.updated_by)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn get_directory_match(
        &self,
        integration_id: &str,
        user_id: &str,
    ) -> Result<Option<DirectoryMatch>, IntegrationError> {
        let Ok(iid) = Uuid::parse_str(integration_id) else {
            return Ok(None);
        };
        let row: Option<(serde_json::Value, serde_json::Value, chrono::DateTime<chrono::Utc>)> =
            sqlx::query_as(
                "SELECT emails, status, checked_at FROM slack_directory_matches
                 WHERE integration_id = $1 AND user_id = $2",
            )
            .bind(iid)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        let Some((emails, status, checked_at)) = row else {
            return Ok(None);
        };
        let (Ok(emails), Ok(status)) = (serde_json::from_value(emails), serde_json::from_value(status))
        else {
            return Ok(None);
        };
        Ok(Some(DirectoryMatch {
            integration_id: integration_id.to_string(),
            user_id: user_id.to_string(),
            emails,
            status,
            checked_at: checked_at.to_rfc3339(),
        }))
    }

    async fn upsert_directory_match(&self, m: &DirectoryMatch) -> Result<(), IntegrationError> {
        let iid = Uuid::parse_str(&m.integration_id)
            .map_err(|_| IntegrationError::NotFound(m.integration_id.clone()))?;
        let checked_at = chrono::DateTime::parse_from_rfc3339(&m.checked_at)
            .map_err(|e| IntegrationError::InvalidInput(e.to_string()))?;
        sqlx::query(
            "INSERT INTO slack_directory_matches (integration_id, user_id, emails, status, checked_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (integration_id, user_id) DO UPDATE SET
               emails = EXCLUDED.emails, status = EXCLUDED.status, checked_at = EXCLUDED.checked_at",
        )
        .bind(iid)
        .bind(&m.user_id)
        .bind(serde_json::to_value(&m.emails).map_err(|e| IntegrationError::Store(e.to_string()))?)
        .bind(serde_json::to_value(&m.status).map_err(|e| IntegrationError::Store(e.to_string()))?)
        .bind(checked_at)
        .execute(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete_directory_matches(&self, integration_id: &str) -> Result<(), IntegrationError> {
        let Ok(iid) = Uuid::parse_str(integration_id) else {
            return Ok(());
        };
        sqlx::query("DELETE FROM slack_directory_matches WHERE integration_id = $1")
            .bind(iid)
            .execute(&self.pool)
            .await
            .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(())
    }

    async fn list_integrations_of_type(
        &self,
        integration_type: IntegrationType,
    ) -> Result<Vec<Integration>, IntegrationError> {
        let rows: Vec<IntegrationRow> = sqlx::query_as(
            "SELECT id, organisation, integration_type, name, config_encrypted, enabled, created_by, created_at, updated_at
             FROM integrations WHERE integration_type = $1 ORDER BY created_at",
        )
        .bind(integration_type.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        rows.into_iter().map(|r| self.row_to_integration(r)).collect()
    }

    async fn bind_github_installation(
        &self,
        installation_id: i64,
        organisation: &str,
        integration_id: &str,
    ) -> Result<(), IntegrationError> {
        let iid = Uuid::parse_str(integration_id)
            .map_err(|_| IntegrationError::NotFound(integration_id.to_string()))?;
        let bound: Option<(Uuid,)> = sqlx::query_as(
            "INSERT INTO github_installations (installation_id, organisation, integration_id)
             VALUES ($1, $2, $3)
             ON CONFLICT (installation_id) DO NOTHING
             RETURNING integration_id",
        )
        .bind(installation_id)
        .bind(organisation)
        .bind(iid)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        if bound.is_some() {
            return Ok(());
        }
        match self.get_github_installation_binding(installation_id).await? {
            Some((_, existing)) if existing == integration_id => Ok(()),
            _ => Err(IntegrationError::Duplicate(
                "GitHub installation is already connected".into(),
            )),
        }
    }

    async fn get_github_installation_binding(
        &self,
        installation_id: i64,
    ) -> Result<Option<(String, String)>, IntegrationError> {
        let row: Option<(String, Uuid)> = sqlx::query_as(
            "SELECT organisation, integration_id FROM github_installations WHERE installation_id = $1",
        )
        .bind(installation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| IntegrationError::Store(e.to_string()))?;
        Ok(row.map(|(org, iid)| (org, iid.to_string())))
    }
}

#[derive(sqlx::FromRow)]
struct SlackUserLinkRow {
    id: Uuid,
    user_id: String,
    team_id: String,
    team_name: String,
    slack_user_id: String,
    slack_username: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct SlackMessageRefRow {
    id: Uuid,
    integration_id: Uuid,
    release_id: String,
    channel_id: String,
    message_ts: String,
    last_event_type: String,
    destinations: serde_json::Value,
    release_title: String,
    thread_keys: serde_json::Value,
    rendered_hash: String,
    destination_count: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// A transaction holding `pg_advisory_xact_lock`: ending the transaction is
/// what releases the lock.
struct PgReleaseLock(sqlx::Transaction<'static, sqlx::Postgres>);

#[async_trait::async_trait]
impl SlackReleaseLock for PgReleaseLock {
    async fn release(self: Box<Self>) {
        if let Err(e) = self.0.commit().await {
            tracing::warn!(error = %e, "releasing slack release lock");
        }
    }
}

#[derive(sqlx::FromRow)]
struct IntegrationRow {
    id: Uuid,
    organisation: String,
    integration_type: String,
    name: String,
    config_encrypted: Vec<u8>,
    enabled: bool,
    created_by: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: Uuid,
    integration_id: Uuid,
    notification_type: String,
    enabled: bool,
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    id: Uuid,
    integration_id: Uuid,
    notification_id: String,
    status: String,
    error_message: Option<String>,
    attempted_at: chrono::DateTime<chrono::Utc>,
}
