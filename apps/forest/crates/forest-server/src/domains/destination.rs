use std::collections::HashMap;

use anyhow::bail;
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// Events
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DestinationEvent {
    Created {
        destination_id: Uuid,
        organisation: String,
        name: String,
        environment: String,
        environment_id: Uuid,
        metadata: HashMap<String, String>,
        type_organisation: String,
        type_name: String,
        type_version: u32,
    },
    MetadataUpdated {
        metadata: HashMap<String, String>,
    },
    Deleted,
}

impl EventData for DestinationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            DestinationEvent::Created { .. } => "destination.created",
            DestinationEvent::MetadataUpdated { .. } => "destination.metadata_updated",
            DestinationEvent::Deleted => "destination.deleted",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationStatus {
    NonExistent,
    Active,
    Deleted,
}

#[derive(Debug)]
pub struct DestinationAggregate {
    pub status: DestinationStatus,
    pub destination_id: Option<Uuid>,
    pub organisation: String,
    pub name: String,
    pub environment: String,
    pub environment_id: Option<Uuid>,
    pub metadata: HashMap<String, String>,
    pub type_organisation: String,
    pub type_name: String,
    pub type_version: u32,
}

impl Default for DestinationAggregate {
    fn default() -> Self {
        Self {
            status: DestinationStatus::NonExistent,
            destination_id: None,
            organisation: String::new(),
            name: String::new(),
            environment: String::new(),
            environment_id: None,
            metadata: HashMap::new(),
            type_organisation: String::new(),
            type_name: String::new(),
            type_version: 0,
        }
    }
}

impl Aggregate for DestinationAggregate {
    type Event = DestinationEvent;

    fn stream_category() -> StreamCategory {
        "destination".into_stream_category()
    }

    fn apply(&mut self, event: &DestinationEvent) {
        match event {
            DestinationEvent::Created {
                destination_id,
                organisation,
                name,
                environment,
                environment_id,
                metadata,
                type_organisation,
                type_name,
                type_version,
            } => {
                self.status = DestinationStatus::Active;
                self.destination_id = Some(*destination_id);
                self.organisation.clone_from(organisation);
                self.name.clone_from(name);
                self.environment.clone_from(environment);
                self.environment_id = Some(*environment_id);
                self.metadata.clone_from(metadata);
                self.type_organisation.clone_from(type_organisation);
                self.type_name.clone_from(type_name);
                self.type_version = *type_version;
            }
            DestinationEvent::MetadataUpdated { metadata } => {
                self.metadata.clone_from(metadata);
            }
            DestinationEvent::Deleted => {
                self.status = DestinationStatus::Deleted;
            }
        }
    }
}

// ============================================================
// Commands (pure business logic)
// ============================================================

pub struct CreateDestinationParams {
    pub organisation: String,
    pub name: String,
    pub environment: String,
    pub environment_id: Uuid,
    pub metadata: HashMap<String, String>,
    pub type_organisation: String,
    pub type_name: String,
    pub type_version: u32,
}

impl DestinationAggregate {
    pub fn create(
        root: &mut AggregateRoot<Self>,
        params: CreateDestinationParams,
    ) -> anyhow::Result<Uuid> {
        match root.state.status {
            DestinationStatus::Active => {
                bail!(
                    "destination {}/{} already exists",
                    params.organisation,
                    params.name
                );
            }
            DestinationStatus::Deleted => {
                bail!(
                    "destination {}/{} has been deleted",
                    params.organisation,
                    params.name
                );
            }
            DestinationStatus::NonExistent => {}
        }

        let destination_id = Uuid::now_v7();

        root.record(DestinationEvent::Created {
            destination_id,
            organisation: params.organisation,
            name: params.name,
            environment: params.environment,
            environment_id: params.environment_id,
            metadata: params.metadata,
            type_organisation: params.type_organisation,
            type_name: params.type_name,
            type_version: params.type_version,
        });

        Ok(destination_id)
    }

    pub fn update_metadata(
        root: &mut AggregateRoot<Self>,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        match root.state.status {
            DestinationStatus::NonExistent => {
                bail!("destination does not exist");
            }
            DestinationStatus::Deleted => {
                bail!("destination has been deleted");
            }
            DestinationStatus::Active => {}
        }

        root.record(DestinationEvent::MetadataUpdated { metadata });

        Ok(())
    }

    pub fn delete(root: &mut AggregateRoot<Self>) -> anyhow::Result<()> {
        match root.state.status {
            DestinationStatus::NonExistent => {
                bail!("destination does not exist");
            }
            DestinationStatus::Deleted => {
                bail!("destination is already deleted");
            }
            DestinationStatus::Active => {}
        }

        root.record(DestinationEvent::Deleted);

        Ok(())
    }
}

/// Stream key for a destination aggregate: `{org}/{name}`
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

    fn new_root() -> AggregateRoot<DestinationAggregate> {
        AggregateRoot::new("destination-acme/prod-k8s".into())
    }

    fn default_params() -> CreateDestinationParams {
        CreateDestinationParams {
            organisation: "acme".into(),
            name: "prod-k8s".into(),
            environment: "production".into(),
            environment_id: Uuid::now_v7(),
            metadata: [("cluster".into(), "us-east-1".into())].into(),
            type_organisation: "forest".into(),
            type_name: "kubernetes".into(),
            type_version: 1,
        }
    }

    // ----------------------------------------------------------
    // Create
    // ----------------------------------------------------------

    #[test]
    fn create_destination_records_event_and_returns_id() {
        let mut root = new_root();
        let params = default_params();
        let env_id = params.environment_id;
        let id = DestinationAggregate::create(&mut root, params).unwrap();

        assert!(id != Uuid::nil());
        assert_eq!(root.state.status, DestinationStatus::Active);
        assert_eq!(root.state.destination_id, Some(id));
        assert_eq!(root.state.organisation, "acme");
        assert_eq!(root.state.name, "prod-k8s");
        assert_eq!(root.state.environment, "production");
        assert_eq!(root.state.environment_id, Some(env_id));
        assert_eq!(root.state.metadata.get("cluster").unwrap(), "us-east-1");
        assert_eq!(root.state.type_organisation, "forest");
        assert_eq!(root.state.type_name, "kubernetes");
        assert_eq!(root.state.type_version, 1);
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn create_rejects_if_already_exists() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();

        let err = DestinationAggregate::create(&mut root, default_params());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn create_rejects_if_deleted() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();
        DestinationAggregate::delete(&mut root).unwrap();

        let err = DestinationAggregate::create(&mut root, default_params());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("deleted"));
    }

    // ----------------------------------------------------------
    // Update metadata
    // ----------------------------------------------------------

    #[test]
    fn update_metadata_replaces_metadata() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();

        let new_meta: HashMap<String, String> =
            [("cluster".into(), "eu-west-1".into()), ("tier".into(), "premium".into())].into();

        DestinationAggregate::update_metadata(&mut root, new_meta.clone()).unwrap();

        assert_eq!(root.state.metadata, new_meta);
        assert_eq!(root.pending_count(), 2);
    }

    #[test]
    fn update_metadata_rejects_non_existent() {
        let mut root = new_root();

        let err = DestinationAggregate::update_metadata(&mut root, HashMap::new());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn update_metadata_rejects_deleted() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();
        DestinationAggregate::delete(&mut root).unwrap();

        let err = DestinationAggregate::update_metadata(&mut root, HashMap::new());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("deleted"));
    }

    #[test]
    fn update_metadata_multiple_times() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();

        let meta1: HashMap<String, String> = [("key".into(), "val1".into())].into();
        DestinationAggregate::update_metadata(&mut root, meta1).unwrap();

        let meta2: HashMap<String, String> = [("key".into(), "val2".into())].into();
        DestinationAggregate::update_metadata(&mut root, meta2.clone()).unwrap();

        assert_eq!(root.state.metadata, meta2);
        assert_eq!(root.pending_count(), 3); // create + 2 updates
    }

    // ----------------------------------------------------------
    // Delete
    // ----------------------------------------------------------

    #[test]
    fn delete_transitions_to_deleted() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();

        DestinationAggregate::delete(&mut root).unwrap();

        assert_eq!(root.state.status, DestinationStatus::Deleted);
        assert_eq!(root.pending_count(), 2);
    }

    #[test]
    fn delete_rejects_non_existent() {
        let mut root = new_root();

        let err = DestinationAggregate::delete(&mut root);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn delete_rejects_already_deleted() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();
        DestinationAggregate::delete(&mut root).unwrap();

        let err = DestinationAggregate::delete(&mut root);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already deleted"));
    }

    // ----------------------------------------------------------
    // Event replay / hydration
    // ----------------------------------------------------------

    #[test]
    fn hydrate_replays_full_lifecycle() {
        let mut root = new_root();
        let id = DestinationAggregate::create(&mut root, default_params()).unwrap();

        let new_meta: HashMap<String, String> = [("updated".into(), "true".into())].into();
        DestinationAggregate::update_metadata(&mut root, new_meta.clone()).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "destination-acme/prod-k8s".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        let replayed = AggregateRoot::<DestinationAggregate>::hydrate(
            "destination-acme/prod-k8s".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, DestinationStatus::Active);
        assert_eq!(replayed.state.destination_id, Some(id));
        assert_eq!(replayed.state.organisation, "acme");
        assert_eq!(replayed.state.name, "prod-k8s");
        assert_eq!(replayed.state.metadata, new_meta);
    }

    #[test]
    fn hydrate_replays_create_then_delete() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();
        DestinationAggregate::delete(&mut root).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "destination-acme/prod-k8s".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        let replayed = AggregateRoot::<DestinationAggregate>::hydrate(
            "destination-acme/prod-k8s".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, DestinationStatus::Deleted);
    }

    #[test]
    fn hydrate_empty_events_gives_non_existent() {
        let root =
            AggregateRoot::<DestinationAggregate>::hydrate("destination-acme/x".into(), &[], 0);
        assert_eq!(root.state.status, DestinationStatus::NonExistent);
    }

    // ----------------------------------------------------------
    // Serde roundtrip
    // ----------------------------------------------------------

    #[test]
    fn event_data_serde_roundtrip() {
        let events = vec![
            DestinationEvent::Created {
                destination_id: Uuid::now_v7(),
                organisation: "acme".into(),
                name: "prod-k8s".into(),
                environment: "production".into(),
                environment_id: Uuid::now_v7(),
                metadata: [("k".into(), "v".into())].into(),
                type_organisation: "forest".into(),
                type_name: "kubernetes".into(),
                type_version: 1,
            },
            DestinationEvent::MetadataUpdated {
                metadata: [("new".into(), "meta".into())].into(),
            },
            DestinationEvent::Deleted,
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let back: DestinationEvent = serde_json::from_value(json).unwrap();
            assert_eq!(event.event_type(), back.event_type());
        }
    }

    // ----------------------------------------------------------
    // Stream category / key
    // ----------------------------------------------------------

    // ----------------------------------------------------------
    // Default state
    // ----------------------------------------------------------

    #[test]
    fn default_aggregate_is_non_existent_with_empty_fields() {
        let root = new_root();
        assert_eq!(root.state.status, DestinationStatus::NonExistent);
        assert_eq!(root.state.destination_id, None);
        assert_eq!(root.state.organisation, "");
        assert_eq!(root.state.name, "");
        assert_eq!(root.state.environment, "");
        assert_eq!(root.state.environment_id, None);
        assert!(root.state.metadata.is_empty());
        assert_eq!(root.state.type_organisation, "");
        assert_eq!(root.state.type_name, "");
        assert_eq!(root.state.type_version, 0);
        assert!(!root.has_pending());
    }

    // ----------------------------------------------------------
    // Empty / edge-case metadata
    // ----------------------------------------------------------

    #[test]
    fn create_with_empty_metadata() {
        let mut root = new_root();
        let params = CreateDestinationParams {
            metadata: HashMap::new(),
            ..default_params()
        };
        let id = DestinationAggregate::create(&mut root, params).unwrap();
        assert!(root.state.metadata.is_empty());
        assert_eq!(root.state.destination_id, Some(id));
    }

    #[test]
    fn update_metadata_to_empty_clears_all() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();
        assert!(!root.state.metadata.is_empty());

        DestinationAggregate::update_metadata(&mut root, HashMap::new()).unwrap();
        assert!(root.state.metadata.is_empty());
    }

    // ----------------------------------------------------------
    // Field preservation
    // ----------------------------------------------------------

    #[test]
    fn update_metadata_does_not_change_other_fields() {
        let mut root = new_root();
        let params = default_params();
        let env_id = params.environment_id;
        let id = DestinationAggregate::create(&mut root, params).unwrap();

        let new_meta: HashMap<String, String> = [("new_key".into(), "new_val".into())].into();
        DestinationAggregate::update_metadata(&mut root, new_meta).unwrap();

        // All non-metadata fields unchanged
        assert_eq!(root.state.destination_id, Some(id));
        assert_eq!(root.state.organisation, "acme");
        assert_eq!(root.state.name, "prod-k8s");
        assert_eq!(root.state.environment, "production");
        assert_eq!(root.state.environment_id, Some(env_id));
        assert_eq!(root.state.type_organisation, "forest");
        assert_eq!(root.state.type_name, "kubernetes");
        assert_eq!(root.state.type_version, 1);
        assert_eq!(root.state.status, DestinationStatus::Active);
    }

    #[test]
    fn delete_does_not_clear_fields() {
        let mut root = new_root();
        let id = DestinationAggregate::create(&mut root, default_params()).unwrap();
        DestinationAggregate::delete(&mut root).unwrap();

        // Fields still present even after deletion (for audit/replay)
        assert_eq!(root.state.destination_id, Some(id));
        assert_eq!(root.state.organisation, "acme");
        assert_eq!(root.state.name, "prod-k8s");
        assert!(!root.state.metadata.is_empty());
    }

    // ----------------------------------------------------------
    // Unique IDs
    // ----------------------------------------------------------

    #[test]
    fn each_create_generates_unique_id() {
        let mut root1 = AggregateRoot::<DestinationAggregate>::new("destination-acme/a".into());
        let mut root2 = AggregateRoot::<DestinationAggregate>::new("destination-acme/b".into());

        let id1 = DestinationAggregate::create(&mut root1, CreateDestinationParams {
            name: "a".into(),
            ..default_params()
        }).unwrap();
        let id2 = DestinationAggregate::create(&mut root2, CreateDestinationParams {
            name: "b".into(),
            ..default_params()
        }).unwrap();

        assert_ne!(id1, id2);
    }

    // ----------------------------------------------------------
    // Full lifecycle hydration
    // ----------------------------------------------------------

    #[test]
    fn hydrate_full_lifecycle_create_update_update_delete() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, default_params()).unwrap();

        let meta1: HashMap<String, String> = [("v".into(), "1".into())].into();
        DestinationAggregate::update_metadata(&mut root, meta1).unwrap();

        let meta2: HashMap<String, String> = [("v".into(), "2".into())].into();
        DestinationAggregate::update_metadata(&mut root, meta2.clone()).unwrap();

        DestinationAggregate::delete(&mut root).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "destination-acme/prod-k8s".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        assert_eq!(events.len(), 4);

        let replayed = AggregateRoot::<DestinationAggregate>::hydrate(
            "destination-acme/prod-k8s".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, DestinationStatus::Deleted);
        assert_eq!(replayed.state.metadata, meta2); // last update wins
        assert_eq!(replayed.state.organisation, "acme");
    }

    #[test]
    fn hydrate_preserves_destination_type_fields() {
        let mut root = new_root();
        DestinationAggregate::create(&mut root, CreateDestinationParams {
            type_organisation: "myorg".into(),
            type_name: "flux".into(),
            type_version: 3,
            ..default_params()
        }).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "destination-acme/prod-k8s".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        let replayed = AggregateRoot::<DestinationAggregate>::hydrate(
            "destination-acme/prod-k8s".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.type_organisation, "myorg");
        assert_eq!(replayed.state.type_name, "flux");
        assert_eq!(replayed.state.type_version, 3);
    }

    // ----------------------------------------------------------
    // Event type strings
    // ----------------------------------------------------------

    #[test]
    fn event_type_strings_are_correct() {
        assert_eq!(
            DestinationEvent::Created {
                destination_id: Uuid::nil(),
                organisation: String::new(),
                name: String::new(),
                environment: String::new(),
                environment_id: Uuid::nil(),
                metadata: HashMap::new(),
                type_organisation: String::new(),
                type_name: String::new(),
                type_version: 0,
            }.event_type(),
            "destination.created"
        );
        assert_eq!(
            DestinationEvent::MetadataUpdated { metadata: HashMap::new() }.event_type(),
            "destination.metadata_updated"
        );
        assert_eq!(DestinationEvent::Deleted.event_type(), "destination.deleted");
    }

    // ----------------------------------------------------------
    // Serde roundtrip — field-level verification
    // ----------------------------------------------------------

    #[test]
    fn serde_roundtrip_created_preserves_all_fields() {
        let dest_id = Uuid::now_v7();
        let env_id = Uuid::now_v7();
        let event = DestinationEvent::Created {
            destination_id: dest_id,
            organisation: "myorg".into(),
            name: "staging-flux".into(),
            environment: "staging".into(),
            environment_id: env_id,
            metadata: [("region".into(), "eu".into()), ("tier".into(), "gold".into())].into(),
            type_organisation: "forest".into(),
            type_name: "flux".into(),
            type_version: 2,
        };

        let json = serde_json::to_value(&event).unwrap();
        let back: DestinationEvent = serde_json::from_value(json).unwrap();

        match back {
            DestinationEvent::Created {
                destination_id,
                organisation,
                name,
                environment,
                environment_id,
                metadata,
                type_organisation,
                type_name,
                type_version,
            } => {
                assert_eq!(destination_id, dest_id);
                assert_eq!(organisation, "myorg");
                assert_eq!(name, "staging-flux");
                assert_eq!(environment, "staging");
                assert_eq!(environment_id, env_id);
                assert_eq!(metadata.len(), 2);
                assert_eq!(metadata.get("region").unwrap(), "eu");
                assert_eq!(metadata.get("tier").unwrap(), "gold");
                assert_eq!(type_organisation, "forest");
                assert_eq!(type_name, "flux");
                assert_eq!(type_version, 2);
            }
            _ => panic!("wrong variant after roundtrip"),
        }
    }

    #[test]
    fn serde_roundtrip_metadata_updated_preserves_fields() {
        let event = DestinationEvent::MetadataUpdated {
            metadata: [("a".into(), "1".into()), ("b".into(), "2".into())].into(),
        };

        let json = serde_json::to_value(&event).unwrap();
        let back: DestinationEvent = serde_json::from_value(json).unwrap();

        match back {
            DestinationEvent::MetadataUpdated { metadata } => {
                assert_eq!(metadata.len(), 2);
                assert_eq!(metadata.get("a").unwrap(), "1");
                assert_eq!(metadata.get("b").unwrap(), "2");
            }
            _ => panic!("wrong variant after roundtrip"),
        }
    }

    #[test]
    fn serde_roundtrip_deleted() {
        let json = serde_json::to_value(&DestinationEvent::Deleted).unwrap();
        let back: DestinationEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back.event_type(), "destination.deleted");
    }

    // ----------------------------------------------------------
    // Apply idempotency / ordering
    // ----------------------------------------------------------

    #[test]
    fn apply_created_overwrites_previous_fields() {
        // Simulates re-applying a Created event on a non-default aggregate
        // (shouldn't happen in practice, but ensures apply is total)
        let mut agg = DestinationAggregate::default();
        agg.organisation = "old".into();
        agg.name = "old-name".into();

        let event = DestinationEvent::Created {
            destination_id: Uuid::now_v7(),
            organisation: "new".into(),
            name: "new-name".into(),
            environment: "prod".into(),
            environment_id: Uuid::now_v7(),
            metadata: HashMap::new(),
            type_organisation: "f".into(),
            type_name: "k".into(),
            type_version: 1,
        };

        agg.apply(&event);
        assert_eq!(agg.organisation, "new");
        assert_eq!(agg.name, "new-name");
        assert_eq!(agg.status, DestinationStatus::Active);
    }

    #[test]
    fn apply_metadata_updated_replaces_entirely() {
        let mut agg = DestinationAggregate::default();
        agg.metadata = [("old".into(), "val".into())].into();

        agg.apply(&DestinationEvent::MetadataUpdated {
            metadata: [("new".into(), "val".into())].into(),
        });

        // Old key is gone — it's a full replacement, not merge
        assert!(agg.metadata.get("old").is_none());
        assert_eq!(agg.metadata.get("new").unwrap(), "val");
    }

    // ----------------------------------------------------------
    // Stream category / key
    // ----------------------------------------------------------

    #[test]
    fn stream_category_is_destination() {
        assert_eq!(DestinationAggregate::stream_category().as_str(), "destination");
    }

    #[test]
    fn stream_key_format() {
        assert_eq!(stream_key("acme", "prod-k8s"), "acme/prod-k8s");
    }
}
