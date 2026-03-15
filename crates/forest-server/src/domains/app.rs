use anyhow::bail;
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// Events
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AppEvent {
    Created {
        app_id: Uuid,
        organisation_id: Uuid,
        name: String,
        description: Option<String>,
        permissions: serde_json::Value,
        created_by: Uuid,
    },
    Suspended,
    Unsuspended,
    Deleted,
    TokenCreated {
        token_id: Uuid,
        name: String,
        expires_at: Option<String>,
    },
    TokenRevoked {
        token_id: Uuid,
    },
}

impl EventData for AppEvent {
    fn event_type(&self) -> &'static str {
        match self {
            AppEvent::Created { .. } => "app.created",
            AppEvent::Suspended => "app.suspended",
            AppEvent::Unsuspended => "app.unsuspended",
            AppEvent::Deleted => "app.deleted",
            AppEvent::TokenCreated { .. } => "app.token_created",
            AppEvent::TokenRevoked { .. } => "app.token_revoked",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppStatus {
    NonExistent,
    Active,
    Deleted,
}

#[derive(Debug)]
pub struct AppAggregate {
    pub status: AppStatus,
    pub app_id: Option<Uuid>,
    pub organisation_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub permissions: serde_json::Value,
    pub suspended: bool,
    pub created_by: Option<Uuid>,
    /// Token IDs tracked for aggregate-level validation.
    pub token_ids: Vec<Uuid>,
}

impl Default for AppAggregate {
    fn default() -> Self {
        Self {
            status: AppStatus::NonExistent,
            app_id: None,
            organisation_id: None,
            name: String::new(),
            description: None,
            permissions: serde_json::Value::Null,
            suspended: false,
            created_by: None,
            token_ids: Vec::new(),
        }
    }
}

impl Aggregate for AppAggregate {
    type Event = AppEvent;

    fn stream_category() -> StreamCategory {
        "app".into_stream_category()
    }

    fn apply(&mut self, event: &AppEvent) {
        match event {
            AppEvent::Created {
                app_id,
                organisation_id,
                name,
                description,
                permissions,
                created_by,
            } => {
                self.status = AppStatus::Active;
                self.app_id = Some(*app_id);
                self.organisation_id = Some(*organisation_id);
                self.name.clone_from(name);
                self.description.clone_from(description);
                self.permissions.clone_from(permissions);
                self.suspended = false;
                self.created_by = Some(*created_by);
            }
            AppEvent::Suspended => {
                self.suspended = true;
            }
            AppEvent::Unsuspended => {
                self.suspended = false;
            }
            AppEvent::Deleted => {
                self.status = AppStatus::Deleted;
            }
            AppEvent::TokenCreated { token_id, .. } => {
                self.token_ids.push(*token_id);
            }
            AppEvent::TokenRevoked { token_id } => {
                self.token_ids.retain(|id| id != token_id);
            }
        }
    }
}

// ============================================================
// Commands (pure business logic)
// ============================================================

pub struct CreateAppParams {
    pub organisation_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub permissions: serde_json::Value,
    pub created_by: Uuid,
}

impl AppAggregate {
    pub fn create(
        root: &mut AggregateRoot<Self>,
        params: CreateAppParams,
    ) -> anyhow::Result<Uuid> {
        match root.state.status {
            AppStatus::Active => bail!("app '{}' already exists", params.name),
            AppStatus::Deleted => bail!("app '{}' has been deleted", params.name),
            AppStatus::NonExistent => {}
        }

        let app_id = Uuid::now_v7();

        root.record(AppEvent::Created {
            app_id,
            organisation_id: params.organisation_id,
            name: params.name,
            description: params.description,
            permissions: params.permissions,
            created_by: params.created_by,
        });

        Ok(app_id)
    }

    pub fn suspend(root: &mut AggregateRoot<Self>) -> anyhow::Result<()> {
        match root.state.status {
            AppStatus::NonExistent => bail!("app does not exist"),
            AppStatus::Deleted => bail!("app has been deleted"),
            AppStatus::Active => {}
        }
        if root.state.suspended {
            return Ok(()); // idempotent
        }
        root.record(AppEvent::Suspended);
        Ok(())
    }

    pub fn unsuspend(root: &mut AggregateRoot<Self>) -> anyhow::Result<()> {
        match root.state.status {
            AppStatus::NonExistent => bail!("app does not exist"),
            AppStatus::Deleted => bail!("app has been deleted"),
            AppStatus::Active => {}
        }
        if !root.state.suspended {
            return Ok(()); // idempotent
        }
        root.record(AppEvent::Unsuspended);
        Ok(())
    }

    pub fn delete(root: &mut AggregateRoot<Self>) -> anyhow::Result<()> {
        match root.state.status {
            AppStatus::NonExistent => bail!("app does not exist"),
            AppStatus::Deleted => bail!("app is already deleted"),
            AppStatus::Active => {}
        }
        root.record(AppEvent::Deleted);
        Ok(())
    }

    pub fn create_token(
        root: &mut AggregateRoot<Self>,
        name: String,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> anyhow::Result<Uuid> {
        match root.state.status {
            AppStatus::NonExistent => bail!("app does not exist"),
            AppStatus::Deleted => bail!("app has been deleted"),
            AppStatus::Active => {}
        }
        if root.state.suspended {
            bail!("cannot create token for suspended app");
        }

        let token_id = Uuid::now_v7();

        root.record(AppEvent::TokenCreated {
            token_id,
            name,
            expires_at: expires_at.map(|dt| dt.to_rfc3339()),
        });

        Ok(token_id)
    }

    pub fn revoke_token(
        root: &mut AggregateRoot<Self>,
        token_id: Uuid,
    ) -> anyhow::Result<()> {
        match root.state.status {
            AppStatus::NonExistent => bail!("app does not exist"),
            AppStatus::Deleted => bail!("app has been deleted"),
            AppStatus::Active => {}
        }

        root.record(AppEvent::TokenRevoked { token_id });
        Ok(())
    }
}

/// Stream key for an app aggregate: `{organisation_id}/{name}`
pub fn stream_key(organisation_id: &Uuid, name: &str) -> String {
    format!("{organisation_id}/{name}")
}

// ============================================================
// Unit tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use forest_event_store::AggregateRoot;

    fn new_root() -> AggregateRoot<AppAggregate> {
        AggregateRoot::new("app-org123/my-app".into())
    }

    fn default_params() -> CreateAppParams {
        CreateAppParams {
            organisation_id: Uuid::now_v7(),
            name: "my-app".into(),
            description: Some("Test app".into()),
            permissions: serde_json::json!(["read", "write"]),
            created_by: Uuid::now_v7(),
        }
    }

    // -- Create --

    #[test]
    fn create_app_records_event() {
        let mut root = new_root();
        let id = AppAggregate::create(&mut root, default_params()).unwrap();
        assert_eq!(root.state.status, AppStatus::Active);
        assert_eq!(root.state.app_id, Some(id));
        assert_eq!(root.state.name, "my-app");
        assert!(!root.state.suspended);
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn create_rejects_duplicate() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();
        assert!(AppAggregate::create(&mut root, default_params()).is_err());
    }

    // -- Suspend --

    #[test]
    fn suspend_and_unsuspend() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();

        AppAggregate::suspend(&mut root).unwrap();
        assert!(root.state.suspended);

        AppAggregate::unsuspend(&mut root).unwrap();
        assert!(!root.state.suspended);
    }

    #[test]
    fn suspend_is_idempotent() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();
        AppAggregate::suspend(&mut root).unwrap();
        let count_before = root.pending_count();
        AppAggregate::suspend(&mut root).unwrap(); // no-op
        assert_eq!(root.pending_count(), count_before);
    }

    // -- Delete --

    #[test]
    fn delete_transitions() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();
        AppAggregate::delete(&mut root).unwrap();
        assert_eq!(root.state.status, AppStatus::Deleted);
    }

    #[test]
    fn delete_rejects_non_existent() {
        let mut root = new_root();
        assert!(AppAggregate::delete(&mut root).is_err());
    }

    // -- Tokens --

    #[test]
    fn create_token_tracks_id() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();
        let tid = AppAggregate::create_token(&mut root, "ci-token".into(), None).unwrap();
        assert!(root.state.token_ids.contains(&tid));
    }

    #[test]
    fn create_token_rejects_suspended() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();
        AppAggregate::suspend(&mut root).unwrap();
        assert!(AppAggregate::create_token(&mut root, "x".into(), None).is_err());
    }

    #[test]
    fn revoke_token_removes_id() {
        let mut root = new_root();
        AppAggregate::create(&mut root, default_params()).unwrap();
        let tid = AppAggregate::create_token(&mut root, "ci-token".into(), None).unwrap();
        AppAggregate::revoke_token(&mut root, tid).unwrap();
        assert!(!root.state.token_ids.contains(&tid));
    }

    // -- Hydration --

    #[test]
    fn hydrate_full_lifecycle() {
        let mut root = new_root();
        let app_id = AppAggregate::create(&mut root, default_params()).unwrap();
        let tid = AppAggregate::create_token(&mut root, "tok".into(), None).unwrap();
        AppAggregate::suspend(&mut root).unwrap();
        AppAggregate::revoke_token(&mut root, tid).unwrap();
        AppAggregate::unsuspend(&mut root).unwrap();
        AppAggregate::delete(&mut root).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "app-org123/my-app".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        assert_eq!(events.len(), 6);

        let replayed = AggregateRoot::<AppAggregate>::hydrate(
            "app-org123/my-app".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, AppStatus::Deleted);
        assert_eq!(replayed.state.app_id, Some(app_id));
        assert!(!replayed.state.suspended);
        assert!(replayed.state.token_ids.is_empty());
    }

    // -- Serde --

    #[test]
    fn event_serde_roundtrip() {
        let events = vec![
            AppEvent::Created {
                app_id: Uuid::now_v7(),
                organisation_id: Uuid::now_v7(),
                name: "test".into(),
                description: None,
                permissions: serde_json::json!([]),
                created_by: Uuid::now_v7(),
            },
            AppEvent::Suspended,
            AppEvent::Unsuspended,
            AppEvent::TokenCreated { token_id: Uuid::now_v7(), name: "ci".into(), expires_at: None },
            AppEvent::TokenRevoked { token_id: Uuid::now_v7() },
            AppEvent::Deleted,
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let back: AppEvent = serde_json::from_value(json).unwrap();
            assert_eq!(event.event_type(), back.event_type());
        }
    }
}
