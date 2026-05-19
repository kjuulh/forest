use anyhow::bail;
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// Events
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PolicyEvent {
    Created {
        policy_id: Uuid,
        project_id: Uuid,
        name: String,
        policy_type: String,
        config: serde_json::Value,
    },
    ConfigUpdated {
        policy_type: String,
        config: serde_json::Value,
    },
    EnabledToggled {
        enabled: bool,
    },
    Deleted,
}

impl EventData for PolicyEvent {
    fn event_type(&self) -> &'static str {
        match self {
            PolicyEvent::Created { .. } => "policy.created",
            PolicyEvent::ConfigUpdated { .. } => "policy.config_updated",
            PolicyEvent::EnabledToggled { .. } => "policy.enabled_toggled",
            PolicyEvent::Deleted => "policy.deleted",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyStatus {
    NonExistent,
    Active,
    Deleted,
}

#[derive(Debug)]
pub struct PolicyAggregate {
    pub status: PolicyStatus,
    pub policy_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub name: String,
    pub enabled: bool,
    pub policy_type: String,
    pub config: serde_json::Value,
}

impl Default for PolicyAggregate {
    fn default() -> Self {
        Self {
            status: PolicyStatus::NonExistent,
            policy_id: None,
            project_id: None,
            name: String::new(),
            enabled: true,
            policy_type: String::new(),
            config: serde_json::Value::Null,
        }
    }
}

impl Aggregate for PolicyAggregate {
    type Event = PolicyEvent;

    fn stream_category() -> StreamCategory {
        "policy".into_stream_category()
    }

    fn apply(&mut self, event: &PolicyEvent) {
        match event {
            PolicyEvent::Created {
                policy_id,
                project_id,
                name,
                policy_type,
                config,
            } => {
                self.status = PolicyStatus::Active;
                self.policy_id = Some(*policy_id);
                self.project_id = Some(*project_id);
                self.name.clone_from(name);
                self.enabled = true;
                self.policy_type.clone_from(policy_type);
                self.config.clone_from(config);
            }
            PolicyEvent::ConfigUpdated {
                policy_type,
                config,
            } => {
                self.policy_type.clone_from(policy_type);
                self.config.clone_from(config);
            }
            PolicyEvent::EnabledToggled { enabled } => {
                self.enabled = *enabled;
            }
            PolicyEvent::Deleted => {
                self.status = PolicyStatus::Deleted;
            }
        }
    }
}

// ============================================================
// Config validation (pure)
// ============================================================

pub fn validate_policy_config(policy_type: &str, config: &serde_json::Value) -> anyhow::Result<()> {
    match policy_type {
        "soak_time" => {
            let source = config.get("source_environment").and_then(|v| v.as_str()).unwrap_or("");
            let target = config.get("target_environment").and_then(|v| v.as_str()).unwrap_or("");
            let duration = config.get("duration_seconds").and_then(|v| v.as_i64()).unwrap_or(0);
            if source.is_empty() {
                bail!("source_environment is required for soak_time policy");
            }
            if target.is_empty() {
                bail!("target_environment is required for soak_time policy");
            }
            if duration <= 0 {
                bail!("duration_seconds must be positive for soak_time policy");
            }
        }
        "branch_restriction" => {
            let target = config.get("target_environment").and_then(|v| v.as_str()).unwrap_or("");
            let pattern = config.get("branch_pattern").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                bail!("target_environment is required for branch_restriction policy");
            }
            if pattern.is_empty() {
                bail!("branch_pattern is required for branch_restriction policy");
            }
            Regex::new(pattern).map_err(|e| anyhow::anyhow!("invalid regex for branch_pattern: {e}"))?;
        }
        "approval" => {
            let target = config.get("target_environment").and_then(|v| v.as_str()).unwrap_or("");
            let required = config.get("required_approvals").and_then(|v| v.as_i64()).unwrap_or(0);
            if target.is_empty() {
                bail!("target_environment is required for approval policy");
            }
            if required < 1 {
                bail!("required_approvals must be >= 1 for approval policy");
            }
        }
        other => bail!("unknown policy type: {other}"),
    }
    Ok(())
}

// ============================================================
// Commands (pure business logic)
// ============================================================

pub struct CreatePolicyParams {
    pub project_id: Uuid,
    pub name: String,
    pub policy_type: String,
    pub config: serde_json::Value,
}

pub struct UpdatePolicyConfigParams {
    pub policy_type: String,
    pub config: serde_json::Value,
}

impl PolicyAggregate {
    pub fn create(
        root: &mut AggregateRoot<Self>,
        params: CreatePolicyParams,
    ) -> anyhow::Result<Uuid> {
        match root.state.status {
            PolicyStatus::Active => bail!("policy '{}' already exists", params.name),
            PolicyStatus::Deleted => bail!("policy '{}' has been deleted", params.name),
            PolicyStatus::NonExistent => {}
        }

        validate_policy_config(&params.policy_type, &params.config)?;

        let policy_id = Uuid::now_v7();

        root.record(PolicyEvent::Created {
            policy_id,
            project_id: params.project_id,
            name: params.name,
            policy_type: params.policy_type,
            config: params.config,
        });

        Ok(policy_id)
    }

    pub fn update_config(
        root: &mut AggregateRoot<Self>,
        params: UpdatePolicyConfigParams,
    ) -> anyhow::Result<()> {
        match root.state.status {
            PolicyStatus::NonExistent => bail!("policy does not exist"),
            PolicyStatus::Deleted => bail!("policy has been deleted"),
            PolicyStatus::Active => {}
        }

        validate_policy_config(&params.policy_type, &params.config)?;

        root.record(PolicyEvent::ConfigUpdated {
            policy_type: params.policy_type,
            config: params.config,
        });

        Ok(())
    }

    pub fn toggle_enabled(
        root: &mut AggregateRoot<Self>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        match root.state.status {
            PolicyStatus::NonExistent => bail!("policy does not exist"),
            PolicyStatus::Deleted => bail!("policy has been deleted"),
            PolicyStatus::Active => {}
        }

        root.record(PolicyEvent::EnabledToggled { enabled });

        Ok(())
    }

    pub fn delete(root: &mut AggregateRoot<Self>) -> anyhow::Result<()> {
        match root.state.status {
            PolicyStatus::NonExistent => bail!("policy does not exist"),
            PolicyStatus::Deleted => bail!("policy is already deleted"),
            PolicyStatus::Active => {}
        }

        root.record(PolicyEvent::Deleted);

        Ok(())
    }
}

/// Stream key for a policy aggregate: `{project_id}/{name}`
pub fn stream_key(project_id: &Uuid, name: &str) -> String {
    format!("{project_id}/{name}")
}

// ============================================================
// Unit tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use forest_event_store::AggregateRoot;

    fn new_root() -> AggregateRoot<PolicyAggregate> {
        AggregateRoot::new("policy-proj123/my-policy".into())
    }

    fn soak_time_config() -> serde_json::Value {
        serde_json::json!({
            "source_environment": "staging",
            "target_environment": "production",
            "duration_seconds": 3600
        })
    }

    fn branch_config() -> serde_json::Value {
        serde_json::json!({
            "target_environment": "production",
            "branch_pattern": "^main$"
        })
    }

    fn approval_config() -> serde_json::Value {
        serde_json::json!({
            "target_environment": "production",
            "required_approvals": 2
        })
    }

    fn default_params() -> CreatePolicyParams {
        CreatePolicyParams {
            project_id: Uuid::now_v7(),
            name: "my-policy".into(),
            policy_type: "soak_time".into(),
            config: soak_time_config(),
        }
    }

    // ----------------------------------------------------------
    // Create
    // ----------------------------------------------------------

    #[test]
    fn create_policy_records_event() {
        let mut root = new_root();
        let id = PolicyAggregate::create(&mut root, default_params()).unwrap();

        assert!(id != Uuid::nil());
        assert_eq!(root.state.status, PolicyStatus::Active);
        assert_eq!(root.state.policy_id, Some(id));
        assert_eq!(root.state.name, "my-policy");
        assert_eq!(root.state.policy_type, "soak_time");
        assert!(root.state.enabled);
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn create_all_policy_types() {
        for (pt, cfg) in [
            ("soak_time", soak_time_config()),
            ("branch_restriction", branch_config()),
            ("approval", approval_config()),
        ] {
            let mut root = AggregateRoot::new(format!("policy-x/{pt}"));
            let params = CreatePolicyParams {
                project_id: Uuid::now_v7(),
                name: pt.into(),
                policy_type: pt.into(),
                config: cfg,
            };
            assert!(PolicyAggregate::create(&mut root, params).is_ok());
        }
    }

    #[test]
    fn create_rejects_if_already_exists() {
        let mut root = new_root();
        PolicyAggregate::create(&mut root, default_params()).unwrap();
        let err = PolicyAggregate::create(&mut root, default_params());
        assert!(err.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn create_rejects_invalid_soak_time() {
        let mut root = new_root();
        let params = CreatePolicyParams {
            config: serde_json::json!({"source_environment": "", "target_environment": "prod", "duration_seconds": 100}),
            ..default_params()
        };
        assert!(PolicyAggregate::create(&mut root, params).is_err());
    }

    #[test]
    fn create_rejects_invalid_branch_pattern() {
        let mut root = new_root();
        let params = CreatePolicyParams {
            policy_type: "branch_restriction".into(),
            config: serde_json::json!({"target_environment": "prod", "branch_pattern": "[invalid"}),
            ..default_params()
        };
        assert!(PolicyAggregate::create(&mut root, params).is_err());
    }

    #[test]
    fn create_rejects_unknown_type() {
        let mut root = new_root();
        let params = CreatePolicyParams {
            policy_type: "unknown".into(),
            config: serde_json::json!({}),
            ..default_params()
        };
        assert!(PolicyAggregate::create(&mut root, params).is_err());
    }

    // ----------------------------------------------------------
    // Update config
    // ----------------------------------------------------------

    #[test]
    fn update_config_changes_type_and_config() {
        let mut root = new_root();
        PolicyAggregate::create(&mut root, default_params()).unwrap();

        PolicyAggregate::update_config(
            &mut root,
            UpdatePolicyConfigParams {
                policy_type: "branch_restriction".into(),
                config: branch_config(),
            },
        )
        .unwrap();

        assert_eq!(root.state.policy_type, "branch_restriction");
        assert_eq!(root.pending_count(), 2);
    }

    #[test]
    fn update_config_rejects_non_existent() {
        let mut root = new_root();
        let err = PolicyAggregate::update_config(
            &mut root,
            UpdatePolicyConfigParams {
                policy_type: "soak_time".into(),
                config: soak_time_config(),
            },
        );
        assert!(err.unwrap_err().to_string().contains("does not exist"));
    }

    // ----------------------------------------------------------
    // Toggle enabled
    // ----------------------------------------------------------

    #[test]
    fn toggle_enabled() {
        let mut root = new_root();
        PolicyAggregate::create(&mut root, default_params()).unwrap();
        assert!(root.state.enabled);

        PolicyAggregate::toggle_enabled(&mut root, false).unwrap();
        assert!(!root.state.enabled);

        PolicyAggregate::toggle_enabled(&mut root, true).unwrap();
        assert!(root.state.enabled);
    }

    // ----------------------------------------------------------
    // Delete
    // ----------------------------------------------------------

    #[test]
    fn delete_transitions_to_deleted() {
        let mut root = new_root();
        PolicyAggregate::create(&mut root, default_params()).unwrap();
        PolicyAggregate::delete(&mut root).unwrap();
        assert_eq!(root.state.status, PolicyStatus::Deleted);
    }

    #[test]
    fn delete_rejects_non_existent() {
        let mut root = new_root();
        assert!(PolicyAggregate::delete(&mut root).is_err());
    }

    #[test]
    fn delete_rejects_already_deleted() {
        let mut root = new_root();
        PolicyAggregate::create(&mut root, default_params()).unwrap();
        PolicyAggregate::delete(&mut root).unwrap();
        assert!(PolicyAggregate::delete(&mut root).unwrap_err().to_string().contains("already deleted"));
    }

    // ----------------------------------------------------------
    // Event replay
    // ----------------------------------------------------------

    #[test]
    fn hydrate_full_lifecycle() {
        let mut root = new_root();
        PolicyAggregate::create(&mut root, default_params()).unwrap();
        PolicyAggregate::toggle_enabled(&mut root, false).unwrap();
        PolicyAggregate::update_config(
            &mut root,
            UpdatePolicyConfigParams {
                policy_type: "branch_restriction".into(),
                config: branch_config(),
            },
        )
        .unwrap();
        PolicyAggregate::delete(&mut root).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "policy-proj123/my-policy".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        assert_eq!(events.len(), 4);

        let replayed = AggregateRoot::<PolicyAggregate>::hydrate(
            "policy-proj123/my-policy".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, PolicyStatus::Deleted);
        assert!(!replayed.state.enabled);
        assert_eq!(replayed.state.policy_type, "branch_restriction");
    }

    // ----------------------------------------------------------
    // Serde roundtrip
    // ----------------------------------------------------------

    #[test]
    fn event_serde_roundtrip() {
        let events = vec![
            PolicyEvent::Created {
                policy_id: Uuid::now_v7(),
                project_id: Uuid::now_v7(),
                name: "test".into(),
                policy_type: "soak_time".into(),
                config: soak_time_config(),
            },
            PolicyEvent::ConfigUpdated {
                policy_type: "branch_restriction".into(),
                config: branch_config(),
            },
            PolicyEvent::EnabledToggled { enabled: false },
            PolicyEvent::Deleted,
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let back: PolicyEvent = serde_json::from_value(json).unwrap();
            assert_eq!(event.event_type(), back.event_type());
        }
    }

    // ----------------------------------------------------------
    // Validation
    // ----------------------------------------------------------

    #[test]
    fn validate_soak_time_requires_positive_duration() {
        let cfg = serde_json::json!({"source_environment": "stg", "target_environment": "prod", "duration_seconds": 0});
        assert!(validate_policy_config("soak_time", &cfg).is_err());
    }

    #[test]
    fn validate_approval_requires_min_one() {
        let cfg = serde_json::json!({"target_environment": "prod", "required_approvals": 0});
        assert!(validate_policy_config("approval", &cfg).is_err());
    }
}
