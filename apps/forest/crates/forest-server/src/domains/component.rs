use std::collections::HashMap;

use anyhow::{bail, Context};
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// Events
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ComponentEvent {
    UploadStarted {
        upload_id: Uuid,
        version: String,
        organisation: String,
        name: String,
    },
    FileUploaded {
        upload_id: Uuid,
        file_path: String,
    },
    VersionPublished {
        upload_id: Uuid,
        version: String,
    },
    UploadAborted {
        upload_id: Uuid,
        reason: String,
    },
}

impl EventData for ComponentEvent {
    fn event_type(&self) -> &'static str {
        match self {
            ComponentEvent::UploadStarted { .. } => "component.upload_started",
            ComponentEvent::FileUploaded { .. } => "component.file_uploaded",
            ComponentEvent::VersionPublished { .. } => "component.version_published",
            ComponentEvent::UploadAborted { .. } => "component.upload_aborted",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionState {
    Uploading { upload_id: Uuid },
    Published,
}

#[derive(Debug, Default)]
pub struct ComponentAggregate {
    pub organisation: String,
    pub name: String,
    /// version string → state
    pub versions: HashMap<String, VersionState>,
}

impl Aggregate for ComponentAggregate {
    type Event = ComponentEvent;

    fn stream_category() -> StreamCategory {
        "component".into_stream_category()
    }

    fn apply(&mut self, event: &ComponentEvent) {
        match event {
            ComponentEvent::UploadStarted {
                upload_id,
                version,
                organisation,
                name,
            } => {
                self.organisation.clone_from(organisation);
                self.name.clone_from(name);
                self.versions.insert(
                    version.clone(),
                    VersionState::Uploading {
                        upload_id: *upload_id,
                    },
                );
            }
            ComponentEvent::FileUploaded { .. } => {
                // No state change — files tracked in projection table
            }
            ComponentEvent::VersionPublished { version, .. } => {
                self.versions.insert(version.clone(), VersionState::Published);
            }
            ComponentEvent::UploadAborted { upload_id, .. } => {
                self.versions
                    .retain(|_, v| !matches!(v, VersionState::Uploading { upload_id: id } if id == upload_id));
            }
        }
    }
}

// ============================================================
// Commands (pure business logic)
// ============================================================

impl ComponentAggregate {
    /// Validate and record an upload start.
    pub fn begin_upload(
        root: &mut AggregateRoot<Self>,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> anyhow::Result<Uuid> {
        if root.state.versions.get(version) == Some(&VersionState::Published) {
            bail!(
                "component {}/{} version {} is already published",
                organisation,
                name,
                version
            );
        }

        // Abort any in-flight upload for this version
        if let Some(VersionState::Uploading { upload_id }) =
            root.state.versions.get(version).cloned()
        {
            root.record(ComponentEvent::UploadAborted {
                upload_id,
                reason: "superseded by new upload".into(),
            });
        }

        let upload_id = Uuid::now_v7();
        root.record(ComponentEvent::UploadStarted {
            upload_id,
            version: version.to_string(),
            organisation: organisation.to_string(),
            name: name.to_string(),
        });

        Ok(upload_id)
    }

    /// Record that a file was uploaded (audit trail).
    pub fn upload_file(
        root: &mut AggregateRoot<Self>,
        upload_id: Uuid,
        file_path: &str,
    ) -> anyhow::Result<()> {
        let is_active = root
            .state
            .versions
            .values()
            .any(|v| matches!(v, VersionState::Uploading { upload_id: id } if *id == upload_id));

        if !is_active {
            bail!("upload {} is not active", upload_id);
        }

        root.record(ComponentEvent::FileUploaded {
            upload_id,
            file_path: file_path.to_string(),
        });

        Ok(())
    }

    /// Publish a version (commit the upload).
    pub fn publish_version(
        root: &mut AggregateRoot<Self>,
        upload_id: Uuid,
    ) -> anyhow::Result<String> {
        let version = root
            .state
            .versions
            .iter()
            .find_map(|(v, state)| match state {
                VersionState::Uploading { upload_id: id } if *id == upload_id => Some(v.clone()),
                _ => None,
            })
            .with_context(|| format!("upload {} is not active", upload_id))?;

        root.record(ComponentEvent::VersionPublished {
            upload_id,
            version: version.clone(),
        });

        Ok(version)
    }
}

/// Stream key for a component aggregate: `{org}/{name}`
pub fn stream_key(organisation: &str, name: &str) -> String {
    format!("{organisation}/{name}")
}

// ============================================================
// Unit tests — pure aggregate logic, no database
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use forest_event_store::AggregateRoot;

    fn new_root() -> AggregateRoot<ComponentAggregate> {
        AggregateRoot::new("component-acme/widget".into())
    }

    #[test]
    fn begin_upload_records_event_and_returns_id() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        assert_eq!(root.state.organisation, "acme");
        assert_eq!(root.state.name, "widget");
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading { upload_id: id })
        );
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn begin_upload_rejects_already_published_version() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id).unwrap();

        let err = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0");
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("already published")
        );
    }

    #[test]
    fn begin_upload_aborts_inflight_for_same_version() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        assert_ne!(id1, id2);
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading { upload_id: id2 })
        );
        // 3 events: UploadStarted, UploadAborted, UploadStarted
        assert_eq!(root.pending_count(), 3);
    }

    #[test]
    fn upload_file_validates_active_upload() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        ComponentAggregate::upload_file(&mut root, id, "deployment.yaml").unwrap();
        assert_eq!(root.pending_count(), 2);

        let bogus = Uuid::now_v7();
        let err = ComponentAggregate::upload_file(&mut root, bogus, "file.txt");
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not active"));
    }

    #[test]
    fn upload_file_rejects_after_publish() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id).unwrap();

        let err = ComponentAggregate::upload_file(&mut root, id, "file.txt");
        assert!(err.is_err());
    }

    #[test]
    fn publish_version_transitions_to_published() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();
        ComponentAggregate::upload_file(&mut root, id, "app.yaml").unwrap();

        let version = ComponentAggregate::publish_version(&mut root, id).unwrap();
        assert_eq!(version, "2.0.0");
        assert_eq!(
            root.state.versions.get("2.0.0"),
            Some(&VersionState::Published)
        );
    }

    #[test]
    fn publish_rejects_unknown_upload() {
        let mut root = new_root();
        let bogus = Uuid::now_v7();
        let err = ComponentAggregate::publish_version(&mut root, bogus);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not active"));
    }

    #[test]
    fn multiple_versions_coexist() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id1).unwrap();

        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();

        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
        assert_eq!(
            root.state.versions.get("2.0.0"),
            Some(&VersionState::Uploading { upload_id: id2 })
        );
    }

    #[test]
    fn event_data_serde_roundtrip() {
        let events = vec![
            ComponentEvent::UploadStarted {
                upload_id: Uuid::now_v7(),
                version: "1.0.0".into(),
                organisation: "acme".into(),
                name: "widget".into(),
            },
            ComponentEvent::FileUploaded {
                upload_id: Uuid::now_v7(),
                file_path: "deploy.yaml".into(),
            },
            ComponentEvent::VersionPublished {
                upload_id: Uuid::now_v7(),
                version: "1.0.0".into(),
            },
            ComponentEvent::UploadAborted {
                upload_id: Uuid::now_v7(),
                reason: "superseded".into(),
            },
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let back: ComponentEvent = serde_json::from_value(json).unwrap();
            assert_eq!(event.event_type(), back.event_type());
        }
    }

    #[test]
    fn hydrate_replays_full_lifecycle() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::upload_file(&mut root, id1, "a.yaml").unwrap();
        ComponentAggregate::publish_version(&mut root, id1).unwrap();
        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                forest_event_store::RecordedEvent {
                    global_position: i as i64 + 1,
                    stream_id: "component-acme/widget".into(),
                    stream_version: i as i64 + 1,
                    event_type: e.event_type().into(),
                    data: serde_json::to_value(&e).unwrap(),
                    metadata: serde_json::json!({}),
                    created_at: chrono::Utc::now(),
                }
            })
            .collect();

        let replayed = AggregateRoot::<ComponentAggregate>::hydrate(
            "component-acme/widget".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.organisation, "acme");
        assert_eq!(replayed.state.name, "widget");
        assert_eq!(
            replayed.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
        assert_eq!(
            replayed.state.versions.get("2.0.0"),
            Some(&VersionState::Uploading { upload_id: id2 })
        );
    }

    #[test]
    fn abort_removes_inflight_version() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        root.record(ComponentEvent::UploadAborted {
            upload_id: id,
            reason: "cancelled by user".into(),
        });

        assert!(root.state.versions.get("1.0.0").is_none());
    }

    #[test]
    fn abort_does_not_affect_other_versions() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();

        root.record(ComponentEvent::UploadAborted {
            upload_id: id1,
            reason: "cancelled".into(),
        });

        assert!(root.state.versions.get("1.0.0").is_none());
        assert_eq!(
            root.state.versions.get("2.0.0"),
            Some(&VersionState::Uploading { upload_id: id2 })
        );
    }

    #[test]
    fn stream_category_is_component() {
        assert_eq!(ComponentAggregate::stream_category().as_str(), "component");
    }
}
