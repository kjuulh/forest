use anyhow::bail;
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// Helper types
// ============================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriggerPatterns {
    pub branch: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub commit_message: Option<String>,
    pub source_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriggerTargets {
    pub environments: Vec<String>,
    pub destinations: Vec<String>,
}

// ============================================================
// Events
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TriggerEvent {
    Created {
        trigger_id: Uuid,
        project_id: Uuid,
        name: String,
        patterns: TriggerPatterns,
        targets: TriggerTargets,
        force_release: bool,
        use_pipeline: bool,
    },
    Updated {
        patterns: Option<TriggerPatterns>,
        targets: Option<TriggerTargets>,
        force_release: Option<bool>,
        use_pipeline: Option<bool>,
    },
    EnabledToggled {
        enabled: bool,
    },
    Deleted,
}

impl EventData for TriggerEvent {
    fn event_type(&self) -> &'static str {
        match self {
            TriggerEvent::Created { .. } => "trigger.created",
            TriggerEvent::Updated { .. } => "trigger.updated",
            TriggerEvent::EnabledToggled { .. } => "trigger.enabled_toggled",
            TriggerEvent::Deleted => "trigger.deleted",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerStatus {
    NonExistent,
    Active,
    Deleted,
}

#[derive(Debug)]
pub struct TriggerAggregate {
    pub status: TriggerStatus,
    pub trigger_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub name: String,
    pub enabled: bool,
    pub patterns: TriggerPatterns,
    pub targets: TriggerTargets,
    pub force_release: bool,
    pub use_pipeline: bool,
}

impl Default for TriggerAggregate {
    fn default() -> Self {
        Self {
            status: TriggerStatus::NonExistent,
            trigger_id: None,
            project_id: None,
            name: String::new(),
            enabled: true,
            patterns: TriggerPatterns::default(),
            targets: TriggerTargets::default(),
            force_release: false,
            use_pipeline: false,
        }
    }
}

impl Aggregate for TriggerAggregate {
    type Event = TriggerEvent;

    fn stream_category() -> StreamCategory {
        "trigger".into_stream_category()
    }

    fn apply(&mut self, event: &TriggerEvent) {
        match event {
            TriggerEvent::Created {
                trigger_id,
                project_id,
                name,
                patterns,
                targets,
                force_release,
                use_pipeline,
            } => {
                self.status = TriggerStatus::Active;
                self.trigger_id = Some(*trigger_id);
                self.project_id = Some(*project_id);
                self.name.clone_from(name);
                self.enabled = true;
                self.patterns.clone_from(patterns);
                self.targets.clone_from(targets);
                self.force_release = *force_release;
                self.use_pipeline = *use_pipeline;
            }
            TriggerEvent::Updated {
                patterns,
                targets,
                force_release,
                use_pipeline,
            } => {
                if let Some(p) = patterns {
                    self.patterns.clone_from(p);
                }
                if let Some(t) = targets {
                    self.targets.clone_from(t);
                }
                if let Some(f) = force_release {
                    self.force_release = *f;
                }
                if let Some(u) = use_pipeline {
                    self.use_pipeline = *u;
                }
            }
            TriggerEvent::EnabledToggled { enabled } => {
                self.enabled = *enabled;
            }
            TriggerEvent::Deleted => {
                self.status = TriggerStatus::Deleted;
            }
        }
    }
}

// ============================================================
// Commands (pure business logic)
// ============================================================

pub struct CreateTriggerParams {
    pub project_id: Uuid,
    pub name: String,
    pub patterns: TriggerPatterns,
    pub targets: TriggerTargets,
    pub force_release: bool,
    pub use_pipeline: bool,
}

pub struct UpdateTriggerParams {
    pub patterns: Option<TriggerPatterns>,
    pub targets: Option<TriggerTargets>,
    pub force_release: Option<bool>,
    pub use_pipeline: Option<bool>,
}

impl TriggerAggregate {
    pub fn create(
        root: &mut AggregateRoot<Self>,
        params: CreateTriggerParams,
    ) -> anyhow::Result<Uuid> {
        match root.state.status {
            TriggerStatus::Active => {
                bail!("trigger '{}' already exists", params.name);
            }
            TriggerStatus::Deleted => {
                bail!("trigger '{}' has been deleted", params.name);
            }
            TriggerStatus::NonExistent => {}
        }

        validate_patterns(&params.patterns)?;

        if !params.use_pipeline
            && params.targets.environments.is_empty()
            && params.targets.destinations.is_empty()
        {
            bail!("at least one target_environment or target_destination is required (or use_pipeline=true)");
        }

        let trigger_id = Uuid::now_v7();

        root.record(TriggerEvent::Created {
            trigger_id,
            project_id: params.project_id,
            name: params.name,
            patterns: params.patterns,
            targets: params.targets,
            force_release: params.force_release,
            use_pipeline: params.use_pipeline,
        });

        Ok(trigger_id)
    }

    pub fn update(
        root: &mut AggregateRoot<Self>,
        params: UpdateTriggerParams,
    ) -> anyhow::Result<()> {
        match root.state.status {
            TriggerStatus::NonExistent => bail!("trigger does not exist"),
            TriggerStatus::Deleted => bail!("trigger has been deleted"),
            TriggerStatus::Active => {}
        }

        if let Some(ref p) = params.patterns {
            validate_patterns(p)?;
        }

        root.record(TriggerEvent::Updated {
            patterns: params.patterns,
            targets: params.targets,
            force_release: params.force_release,
            use_pipeline: params.use_pipeline,
        });

        Ok(())
    }

    pub fn toggle_enabled(
        root: &mut AggregateRoot<Self>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        match root.state.status {
            TriggerStatus::NonExistent => bail!("trigger does not exist"),
            TriggerStatus::Deleted => bail!("trigger has been deleted"),
            TriggerStatus::Active => {}
        }

        root.record(TriggerEvent::EnabledToggled { enabled });

        Ok(())
    }

    pub fn delete(root: &mut AggregateRoot<Self>) -> anyhow::Result<()> {
        match root.state.status {
            TriggerStatus::NonExistent => bail!("trigger does not exist"),
            TriggerStatus::Deleted => bail!("trigger is already deleted"),
            TriggerStatus::Active => {}
        }

        root.record(TriggerEvent::Deleted);

        Ok(())
    }
}

// ============================================================
// Pattern matching (pure domain logic)
// ============================================================

/// Data extracted from an annotation, used to evaluate triggers.
pub struct AnnotationMatchData {
    pub branch: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub commit_message: Option<String>,
    pub source_type: Option<String>,
}

impl AnnotationMatchData {
    pub fn from_parts(
        source: &crate::services::release_registry::Source,
        context: &crate::services::release_registry::ArtifactContext,
        reference: &crate::services::release_registry::Reference,
    ) -> Self {
        Self {
            branch: reference.commit_branch.clone(),
            title: context.title.clone(),
            author: source.username.clone(),
            commit_message: reference.commit_message.clone(),
            source_type: source.source_type.clone(),
        }
    }
}

/// Result of evaluating triggers — which triggers matched and what to release to.
pub struct TriggerMatch {
    pub trigger_name: String,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
}

pub fn matches_trigger(patterns: &TriggerPatterns, data: &AnnotationMatchData) -> bool {
    check_pattern(&patterns.branch, data.branch.as_deref())
        && check_pattern(&patterns.title, Some(&data.title))
        && check_pattern(&patterns.author, data.author.as_deref())
        && check_pattern(&patterns.commit_message, data.commit_message.as_deref())
        && check_pattern(&patterns.source_type, data.source_type.as_deref())
}

fn check_pattern(pattern: &Option<String>, value: Option<&str>) -> bool {
    match (pattern, value) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(p), Some(v)) => match Regex::new(p) {
            Ok(re) => re.is_match(v),
            Err(e) => {
                tracing::warn!(pattern = p, "invalid regex in trigger: {e}");
                false
            }
        },
    }
}

fn validate_optional_regex(pattern: &Option<String>, field: &str) -> anyhow::Result<()> {
    if let Some(p) = pattern {
        Regex::new(p).map_err(|e| anyhow::anyhow!("invalid regex for {field}: {e}"))?;
    }
    Ok(())
}

fn validate_patterns(patterns: &TriggerPatterns) -> anyhow::Result<()> {
    validate_optional_regex(&patterns.branch, "branch_pattern")?;
    validate_optional_regex(&patterns.title, "title_pattern")?;
    validate_optional_regex(&patterns.author, "author_pattern")?;
    validate_optional_regex(&patterns.commit_message, "commit_message_pattern")?;
    validate_optional_regex(&patterns.source_type, "source_type_pattern")?;
    Ok(())
}

/// Stream key for a trigger aggregate: `{project_id}/{name}`
pub fn stream_key(project_id: &Uuid, name: &str) -> String {
    format!("{project_id}/{name}")
}

// ============================================================
// Unit tests — pure aggregate logic, no database
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use forest_event_store::AggregateRoot;

    fn new_root() -> AggregateRoot<TriggerAggregate> {
        AggregateRoot::new("trigger-proj123/my-trigger".into())
    }

    fn default_params() -> CreateTriggerParams {
        CreateTriggerParams {
            project_id: Uuid::now_v7(),
            name: "my-trigger".into(),
            patterns: TriggerPatterns {
                branch: Some("^main$".into()),
                title: None,
                author: None,
                commit_message: None,
                source_type: None,
            },
            targets: TriggerTargets {
                environments: vec!["production".into()],
                destinations: vec![],
            },
            force_release: false,
            use_pipeline: false,
        }
    }

    // ----------------------------------------------------------
    // Create
    // ----------------------------------------------------------

    #[test]
    fn create_trigger_records_event_and_returns_id() {
        let mut root = new_root();
        let params = default_params();
        let project_id = params.project_id;
        let id = TriggerAggregate::create(&mut root, params).unwrap();

        assert!(id != Uuid::nil());
        assert_eq!(root.state.status, TriggerStatus::Active);
        assert_eq!(root.state.trigger_id, Some(id));
        assert_eq!(root.state.project_id, Some(project_id));
        assert_eq!(root.state.name, "my-trigger");
        assert!(root.state.enabled);
        assert_eq!(root.state.patterns.branch.as_deref(), Some("^main$"));
        assert_eq!(root.state.targets.environments, vec!["production"]);
        assert!(!root.state.force_release);
        assert!(!root.state.use_pipeline);
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn create_rejects_if_already_exists() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();

        let err = TriggerAggregate::create(&mut root, default_params());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn create_rejects_if_deleted() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();
        TriggerAggregate::delete(&mut root).unwrap();

        let err = TriggerAggregate::create(&mut root, default_params());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("deleted"));
    }

    #[test]
    fn create_rejects_invalid_regex() {
        let mut root = new_root();
        let params = CreateTriggerParams {
            patterns: TriggerPatterns {
                branch: Some("[invalid".into()),
                ..TriggerPatterns::default()
            },
            ..default_params()
        };
        let err = TriggerAggregate::create(&mut root, params);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("invalid regex"));
    }

    #[test]
    fn create_rejects_no_targets() {
        let mut root = new_root();
        let params = CreateTriggerParams {
            targets: TriggerTargets::default(),
            use_pipeline: false,
            ..default_params()
        };
        let err = TriggerAggregate::create(&mut root, params);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("at least one"));
    }

    #[test]
    fn create_allows_no_targets_with_use_pipeline() {
        let mut root = new_root();
        let params = CreateTriggerParams {
            targets: TriggerTargets::default(),
            use_pipeline: true,
            ..default_params()
        };
        assert!(TriggerAggregate::create(&mut root, params).is_ok());
    }

    // ----------------------------------------------------------
    // Update
    // ----------------------------------------------------------

    #[test]
    fn update_changes_patterns() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();

        let new_patterns = TriggerPatterns {
            branch: Some("^release/.*".into()),
            title: Some(".*deploy.*".into()),
            ..TriggerPatterns::default()
        };
        TriggerAggregate::update(
            &mut root,
            UpdateTriggerParams {
                patterns: Some(new_patterns.clone()),
                targets: None,
                force_release: None,
                use_pipeline: None,
            },
        )
        .unwrap();

        assert_eq!(root.state.patterns, new_patterns);
        assert_eq!(root.pending_count(), 2);
    }

    #[test]
    fn update_changes_targets() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();

        let new_targets = TriggerTargets {
            environments: vec!["staging".into(), "production".into()],
            destinations: vec!["dest-1".into()],
        };
        TriggerAggregate::update(
            &mut root,
            UpdateTriggerParams {
                patterns: None,
                targets: Some(new_targets.clone()),
                force_release: Some(true),
                use_pipeline: None,
            },
        )
        .unwrap();

        assert_eq!(root.state.targets, new_targets);
        assert!(root.state.force_release);
    }

    #[test]
    fn update_rejects_non_existent() {
        let mut root = new_root();
        let err = TriggerAggregate::update(
            &mut root,
            UpdateTriggerParams {
                patterns: None,
                targets: None,
                force_release: None,
                use_pipeline: None,
            },
        );
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn update_rejects_invalid_regex() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();

        let err = TriggerAggregate::update(
            &mut root,
            UpdateTriggerParams {
                patterns: Some(TriggerPatterns {
                    branch: Some("[bad".into()),
                    ..TriggerPatterns::default()
                }),
                targets: None,
                force_release: None,
                use_pipeline: None,
            },
        );
        assert!(err.is_err());
    }

    // ----------------------------------------------------------
    // Toggle enabled
    // ----------------------------------------------------------

    #[test]
    fn toggle_enabled_disables_trigger() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();
        assert!(root.state.enabled);

        TriggerAggregate::toggle_enabled(&mut root, false).unwrap();
        assert!(!root.state.enabled);

        TriggerAggregate::toggle_enabled(&mut root, true).unwrap();
        assert!(root.state.enabled);
    }

    #[test]
    fn toggle_enabled_rejects_non_existent() {
        let mut root = new_root();
        let err = TriggerAggregate::toggle_enabled(&mut root, false);
        assert!(err.is_err());
    }

    // ----------------------------------------------------------
    // Delete
    // ----------------------------------------------------------

    #[test]
    fn delete_transitions_to_deleted() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();
        TriggerAggregate::delete(&mut root).unwrap();

        assert_eq!(root.state.status, TriggerStatus::Deleted);
        assert_eq!(root.pending_count(), 2);
    }

    #[test]
    fn delete_rejects_non_existent() {
        let mut root = new_root();
        let err = TriggerAggregate::delete(&mut root);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn delete_rejects_already_deleted() {
        let mut root = new_root();
        TriggerAggregate::create(&mut root, default_params()).unwrap();
        TriggerAggregate::delete(&mut root).unwrap();

        let err = TriggerAggregate::delete(&mut root);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already deleted"));
    }

    #[test]
    fn delete_preserves_fields() {
        let mut root = new_root();
        let id = TriggerAggregate::create(&mut root, default_params()).unwrap();
        TriggerAggregate::delete(&mut root).unwrap();

        assert_eq!(root.state.trigger_id, Some(id));
        assert_eq!(root.state.name, "my-trigger");
        assert_eq!(root.state.patterns.branch.as_deref(), Some("^main$"));
    }

    // ----------------------------------------------------------
    // Event replay / hydration
    // ----------------------------------------------------------

    #[test]
    fn hydrate_replays_full_lifecycle() {
        let mut root = new_root();
        let id = TriggerAggregate::create(&mut root, default_params()).unwrap();
        TriggerAggregate::toggle_enabled(&mut root, false).unwrap();
        TriggerAggregate::update(
            &mut root,
            UpdateTriggerParams {
                patterns: Some(TriggerPatterns {
                    branch: Some("^release/.*".into()),
                    ..TriggerPatterns::default()
                }),
                targets: None,
                force_release: Some(true),
                use_pipeline: None,
            },
        )
        .unwrap();
        TriggerAggregate::delete(&mut root).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "trigger-proj123/my-trigger".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        assert_eq!(events.len(), 4);

        let replayed = AggregateRoot::<TriggerAggregate>::hydrate(
            "trigger-proj123/my-trigger".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, TriggerStatus::Deleted);
        assert_eq!(replayed.state.trigger_id, Some(id));
        assert!(!replayed.state.enabled);
        assert_eq!(replayed.state.patterns.branch.as_deref(), Some("^release/.*"));
        assert!(replayed.state.force_release);
    }

    #[test]
    fn hydrate_empty_events_gives_non_existent() {
        let root = AggregateRoot::<TriggerAggregate>::hydrate("trigger-x/y".into(), &[], 0);
        assert_eq!(root.state.status, TriggerStatus::NonExistent);
    }

    // ----------------------------------------------------------
    // Serde roundtrip
    // ----------------------------------------------------------

    #[test]
    fn event_data_serde_roundtrip() {
        let events = vec![
            TriggerEvent::Created {
                trigger_id: Uuid::now_v7(),
                project_id: Uuid::now_v7(),
                name: "test".into(),
                patterns: TriggerPatterns::default(),
                targets: TriggerTargets {
                    environments: vec!["prod".into()],
                    destinations: vec![],
                },
                force_release: false,
                use_pipeline: true,
            },
            TriggerEvent::Updated {
                patterns: Some(TriggerPatterns {
                    branch: Some("main".into()),
                    ..TriggerPatterns::default()
                }),
                targets: None,
                force_release: None,
                use_pipeline: None,
            },
            TriggerEvent::EnabledToggled { enabled: false },
            TriggerEvent::Deleted,
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let back: TriggerEvent = serde_json::from_value(json).unwrap();
            assert_eq!(event.event_type(), back.event_type());
        }
    }

    // ----------------------------------------------------------
    // Pattern matching
    // ----------------------------------------------------------

    #[test]
    fn matches_trigger_all_none_patterns_matches_everything() {
        let patterns = TriggerPatterns::default();
        let data = AnnotationMatchData {
            branch: Some("main".into()),
            title: "deploy".into(),
            author: Some("alice".into()),
            commit_message: Some("fix bug".into()),
            source_type: Some("ci".into()),
        };
        assert!(matches_trigger(&patterns, &data));
    }

    #[test]
    fn matches_trigger_branch_pattern_filters() {
        let patterns = TriggerPatterns {
            branch: Some("^main$".into()),
            ..TriggerPatterns::default()
        };

        let matches_main = AnnotationMatchData {
            branch: Some("main".into()),
            title: "x".into(),
            author: None,
            commit_message: None,
            source_type: None,
        };
        assert!(matches_trigger(&patterns, &matches_main));

        let no_match = AnnotationMatchData {
            branch: Some("develop".into()),
            title: "x".into(),
            author: None,
            commit_message: None,
            source_type: None,
        };
        assert!(!matches_trigger(&patterns, &no_match));
    }

    #[test]
    fn matches_trigger_pattern_with_none_value_returns_false() {
        let patterns = TriggerPatterns {
            branch: Some("main".into()),
            ..TriggerPatterns::default()
        };
        let data = AnnotationMatchData {
            branch: None,
            title: "x".into(),
            author: None,
            commit_message: None,
            source_type: None,
        };
        assert!(!matches_trigger(&patterns, &data));
    }

    // ----------------------------------------------------------
    // Stream key
    // ----------------------------------------------------------

    #[test]
    fn stream_key_format() {
        let id = Uuid::nil();
        assert_eq!(
            stream_key(&id, "my-trigger"),
            format!("{id}/my-trigger")
        );
    }

    // ----------------------------------------------------------
    // Default state
    // ----------------------------------------------------------

    #[test]
    fn default_aggregate_is_non_existent() {
        let root = new_root();
        assert_eq!(root.state.status, TriggerStatus::NonExistent);
        assert_eq!(root.state.trigger_id, None);
        assert!(root.state.enabled);
        assert!(!root.has_pending());
    }
}
