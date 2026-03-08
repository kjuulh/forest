use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

#[derive(Clone)]
pub struct EventSubscriptionRegistry {
    db: PgPool,
}

pub struct SubscriptionRecord {
    pub id: Uuid,
    pub organisation: String,
    pub name: String,
    pub resource_types: Vec<String>,
    pub actions: Vec<String>,
    pub projects: Vec<String>,
    pub status: String,
    pub cursor: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct CreateSubscriptionParams {
    pub organisation: String,
    pub name: String,
    pub resource_types: Vec<String>,
    pub actions: Vec<String>,
    pub projects: Vec<String>,
    pub created_by_app_id: Option<Uuid>,
    pub created_by_user_id: Option<Uuid>,
}

impl EventSubscriptionRegistry {
    pub async fn create(
        &self,
        params: CreateSubscriptionParams,
    ) -> anyhow::Result<SubscriptionRecord> {
        let rec = sqlx::query!(
            r#"INSERT INTO event_subscriptions
                (organisation, name, resource_types, actions, projects,
                 created_by_app_id, created_by_user_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING id, organisation, name, resource_types, actions, projects,
                         status, cursor, created_at, updated_at"#,
            params.organisation,
            params.name,
            &params.resource_types,
            &params.actions,
            &params.projects,
            params.created_by_app_id,
            params.created_by_user_id,
        )
        .fetch_one(&self.db)
        .await
        .context("create event subscription")?;

        Ok(SubscriptionRecord {
            id: rec.id,
            organisation: rec.organisation,
            name: rec.name,
            resource_types: rec.resource_types,
            actions: rec.actions,
            projects: rec.projects,
            status: rec.status,
            cursor: rec.cursor,
            created_at: rec.created_at,
            updated_at: rec.updated_at,
        })
    }

    pub async fn get(
        &self,
        organisation: &str,
        name: &str,
    ) -> anyhow::Result<Option<SubscriptionRecord>> {
        let rec = sqlx::query!(
            r#"SELECT id, organisation, name, resource_types, actions, projects,
                      status, cursor, created_at, updated_at
               FROM event_subscriptions
               WHERE organisation = $1 AND name = $2"#,
            organisation,
            name,
        )
        .fetch_optional(&self.db)
        .await
        .context("get event subscription")?;

        Ok(rec.map(|r| SubscriptionRecord {
            id: r.id,
            organisation: r.organisation,
            name: r.name,
            resource_types: r.resource_types,
            actions: r.actions,
            projects: r.projects,
            status: r.status,
            cursor: r.cursor,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }))
    }

    pub async fn list(&self, organisation: &str) -> anyhow::Result<Vec<SubscriptionRecord>> {
        let recs = sqlx::query!(
            r#"SELECT id, organisation, name, resource_types, actions, projects,
                      status, cursor, created_at, updated_at
               FROM event_subscriptions
               WHERE organisation = $1
               ORDER BY name"#,
            organisation,
        )
        .fetch_all(&self.db)
        .await
        .context("list event subscriptions")?;

        Ok(recs
            .into_iter()
            .map(|r| SubscriptionRecord {
                id: r.id,
                organisation: r.organisation,
                name: r.name,
                resource_types: r.resource_types,
                actions: r.actions,
                projects: r.projects,
                status: r.status,
                cursor: r.cursor,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect())
    }

    pub async fn update(
        &self,
        organisation: &str,
        name: &str,
        status: Option<&str>,
        update_filters: bool,
        resource_types: Vec<String>,
        actions: Vec<String>,
        projects: Vec<String>,
    ) -> anyhow::Result<SubscriptionRecord> {
        let rec = sqlx::query!(
            r#"UPDATE event_subscriptions SET
                status = COALESCE($3, status),
                resource_types = CASE WHEN $4 THEN $5 ELSE resource_types END,
                actions = CASE WHEN $4 THEN $6 ELSE actions END,
                projects = CASE WHEN $4 THEN $7 ELSE projects END,
                updated_at = now()
               WHERE organisation = $1 AND name = $2
               RETURNING id, organisation, name, resource_types, actions, projects,
                         status, cursor, created_at, updated_at"#,
            organisation,
            name,
            status,
            update_filters,
            &resource_types,
            &actions,
            &projects,
        )
        .fetch_one(&self.db)
        .await
        .context("update event subscription")?;

        Ok(SubscriptionRecord {
            id: rec.id,
            organisation: rec.organisation,
            name: rec.name,
            resource_types: rec.resource_types,
            actions: rec.actions,
            projects: rec.projects,
            status: rec.status,
            cursor: rec.cursor,
            created_at: rec.created_at,
            updated_at: rec.updated_at,
        })
    }

    pub async fn delete(&self, organisation: &str, name: &str) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "DELETE FROM event_subscriptions WHERE organisation = $1 AND name = $2",
            organisation,
            name,
        )
        .execute(&self.db)
        .await
        .context("delete event subscription")?;

        if res.rows_affected() == 0 {
            anyhow::bail!("subscription not found");
        }

        Ok(())
    }

    /// Advance the cursor for a subscription. Only moves forward (idempotent).
    pub async fn acknowledge(
        &self,
        organisation: &str,
        name: &str,
        sequence: i64,
    ) -> anyhow::Result<i64> {
        let rec = sqlx::query_scalar!(
            r#"UPDATE event_subscriptions
               SET cursor = GREATEST(cursor, $3), updated_at = now()
               WHERE organisation = $1 AND name = $2
               RETURNING cursor"#,
            organisation,
            name,
            sequence,
        )
        .fetch_one(&self.db)
        .await
        .context("acknowledge events")?;

        Ok(rec)
    }
}

pub trait EventSubscriptionRegistryState {
    fn event_subscription_registry(&self) -> EventSubscriptionRegistry;
}

impl EventSubscriptionRegistryState for State {
    fn event_subscription_registry(&self) -> EventSubscriptionRegistry {
        EventSubscriptionRegistry {
            db: self.db.clone(),
        }
    }
}
