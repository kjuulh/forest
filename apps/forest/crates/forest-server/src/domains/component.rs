use std::collections::HashMap;

use anyhow::{Context, bail};
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
    /// A manifest has been recorded against an in-flight upload.
    /// See TASKS/023-publish-transactional.md — required before commit
    /// (enforcement lands in a follow-up slice).
    ManifestRecorded {
        upload_id: Uuid,
    },
    VersionPublished {
        upload_id: Uuid,
        version: String,
    },
    UploadAborted {
        upload_id: Uuid,
        reason: String,
    },
    /// A previously-published version has been unpublished by an owner
    /// (TASKS/025). The version becomes unreachable for `forest global
    /// add` and `forest components show`. Re-publishing at the same
    /// version is allowed afterward — the event log retains the full
    /// lifecycle for audit.
    VersionUnpublished {
        version: String,
        actor: String,
        reason: Option<String>,
    },
}

impl EventData for ComponentEvent {
    fn event_type(&self) -> &'static str {
        match self {
            ComponentEvent::UploadStarted { .. } => "component.upload_started",
            ComponentEvent::FileUploaded { .. } => "component.file_uploaded",
            ComponentEvent::ManifestRecorded { .. } => "component.manifest_recorded",
            ComponentEvent::VersionPublished { .. } => "component.version_published",
            ComponentEvent::UploadAborted { .. } => "component.upload_aborted",
            ComponentEvent::VersionUnpublished { .. } => "component.version_unpublished",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionState {
    Uploading {
        upload_id: Uuid,
        /// `true` once `ManifestRecorded` has been observed for this upload.
        /// Will become a precondition for `publish_version` in a follow-up
        /// slice of TASKS/023-publish-transactional.md.
        has_manifest: bool,
    },
    Published,
    /// Version was published then later removed by an owner (TASKS/025).
    /// Read paths skip these; a new `begin_upload` for the same version is
    /// allowed and treats the state as if the version had never existed.
    Unpublished,
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
                        has_manifest: false,
                    },
                );
            }
            ComponentEvent::FileUploaded { .. } => {
                // No state change — files tracked in projection table
            }
            ComponentEvent::ManifestRecorded { upload_id } => {
                for state in self.versions.values_mut() {
                    if let VersionState::Uploading {
                        upload_id: id,
                        has_manifest,
                    } = state
                    {
                        if id == upload_id {
                            *has_manifest = true;
                        }
                    }
                }
            }
            ComponentEvent::VersionPublished { version, .. } => {
                self.versions
                    .insert(version.clone(), VersionState::Published);
            }
            ComponentEvent::UploadAborted { upload_id, .. } => {
                self.versions
                    .retain(|_, v| !matches!(v, VersionState::Uploading { upload_id: id, .. } if id == upload_id));
            }
            ComponentEvent::VersionUnpublished { version, .. } => {
                self.versions
                    .insert(version.clone(), VersionState::Unpublished);
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
        // TASKS/025: Unpublished versions are treated like never-existed —
        // a re-publish at the same version is allowed and lands cleanly on
        // top of the unpublish event in the log.
        if root.state.versions.get(version) == Some(&VersionState::Published) {
            bail!(
                "component {}/{} version {} is already published",
                organisation,
                name,
                version
            );
        }

        // Abort any in-flight upload for this version
        if let Some(VersionState::Uploading { upload_id, .. }) =
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
        let is_active = root.state.versions.values().any(
            |v| matches!(v, VersionState::Uploading { upload_id: id, .. } if *id == upload_id),
        );

        if !is_active {
            bail!("upload {} is not active", upload_id);
        }

        root.record(ComponentEvent::FileUploaded {
            upload_id,
            file_path: file_path.to_string(),
        });

        Ok(())
    }

    /// Unpublish a previously-published version (TASKS/025).
    ///
    /// Outcomes:
    /// - Version was `Published`: emits `VersionUnpublished`, transitions to
    ///   `Unpublished`.
    /// - Version was `Unpublished` already: no-op (idempotent), Ok(false).
    /// - Version was `Uploading` or absent: error — only Published versions
    ///   can be unpublished. (In-flight uploads are cleaned up via abort,
    ///   absent versions don't need unpublishing.)
    ///
    /// Returns Ok(true) when a new event was recorded, Ok(false) on no-op.
    pub fn unpublish_version(
        root: &mut AggregateRoot<Self>,
        version: &str,
        actor: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<bool> {
        match root.state.versions.get(version) {
            Some(VersionState::Published) => {
                root.record(ComponentEvent::VersionUnpublished {
                    version: version.to_string(),
                    actor: actor.to_string(),
                    reason: reason.map(str::to_string),
                });
                Ok(true)
            }
            Some(VersionState::Unpublished) => Ok(false), // idempotent
            Some(VersionState::Uploading { .. }) => {
                bail!(
                    "version {} is in-flight, not Published; use abort_upload to cancel",
                    version
                );
            }
            None => bail!("version {} not found", version),
        }
    }

    /// Explicitly abort an in-flight upload (TASKS/023).
    ///
    /// The caller (typically a CLI's `AbortOnDrop` guard) signals that the
    /// upload will not be committed. We emit `UploadAborted`, which removes
    /// the version from `Uploading` state and frees the next `begin_upload`
    /// for the same version to proceed cleanly without superseding.
    ///
    /// Idempotent: aborting an unknown / already-aborted upload is a no-op,
    /// so retries from a flaky network are safe.
    pub fn abort_upload(
        root: &mut AggregateRoot<Self>,
        upload_id: Uuid,
        reason: &str,
    ) -> anyhow::Result<()> {
        let is_active = root.state.versions.values().any(
            |v| matches!(v, VersionState::Uploading { upload_id: id, .. } if *id == upload_id),
        );

        if !is_active {
            return Ok(());
        }

        root.record(ComponentEvent::UploadAborted {
            upload_id,
            reason: reason.to_string(),
        });

        Ok(())
    }

    /// Record that a manifest has been published for an in-flight upload.
    ///
    /// See TASKS/023-publish-transactional.md. This is the aggregate-level
    /// counterpart to the service-layer `publish_manifest`, which writes
    /// the manifest JSON to a projection. Recording the event here lets
    /// `publish_version` later require manifest presence as a precondition.
    pub fn record_manifest(root: &mut AggregateRoot<Self>, upload_id: Uuid) -> anyhow::Result<()> {
        let is_active = root.state.versions.values().any(
            |v| matches!(v, VersionState::Uploading { upload_id: id, .. } if *id == upload_id),
        );

        if !is_active {
            bail!("upload {} is not active", upload_id);
        }

        root.record(ComponentEvent::ManifestRecorded { upload_id });

        Ok(())
    }

    /// Publish a version (commit the upload).
    ///
    /// When `require_manifest` is `true`, refuses to commit unless a
    /// `ManifestRecorded` event has been observed for this upload. The
    /// caller is responsible for setting the flag based on the publish
    /// shape: binary / external kinds MUST have a manifest; CUE-only and
    /// Deno kinds today have no manifest validator and pass `false`.
    /// See TASKS/023-publish-transactional.md.
    pub fn publish_version(
        root: &mut AggregateRoot<Self>,
        upload_id: Uuid,
        require_manifest: bool,
    ) -> anyhow::Result<String> {
        let (version, has_manifest) = root
            .state
            .versions
            .iter()
            .find_map(|(v, state)| match state {
                VersionState::Uploading {
                    upload_id: id,
                    has_manifest,
                } if *id == upload_id => Some((v.clone(), *has_manifest)),
                _ => None,
            })
            .with_context(|| format!("upload {} is not active", upload_id))?;

        if require_manifest && !has_manifest {
            bail!(
                "upload {} cannot be committed: no manifest recorded \
                 (call publish_manifest before commit_upload)",
                upload_id
            );
        }

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
            Some(&VersionState::Uploading {
                upload_id: id,
                has_manifest: false
            })
        );
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn begin_upload_rejects_already_published_version() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();

        let err = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0");
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already published"));
    }

    #[test]
    fn begin_upload_aborts_inflight_for_same_version() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        assert_ne!(id1, id2);
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id2,
                has_manifest: false
            })
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
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();

        let err = ComponentAggregate::upload_file(&mut root, id, "file.txt");
        assert!(err.is_err());
    }

    #[test]
    fn publish_version_transitions_to_published() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();
        ComponentAggregate::upload_file(&mut root, id, "app.yaml").unwrap();

        let version = ComponentAggregate::publish_version(&mut root, id, false).unwrap();
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
        let err = ComponentAggregate::publish_version(&mut root, bogus, false);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not active"));
    }

    #[test]
    fn multiple_versions_coexist() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id1, false).unwrap();

        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();

        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
        assert_eq!(
            root.state.versions.get("2.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id2,
                has_manifest: false
            })
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
        ComponentAggregate::publish_version(&mut root, id1, false).unwrap();
        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "2.0.0").unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "component-acme/widget".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
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
            Some(&VersionState::Uploading {
                upload_id: id2,
                has_manifest: false
            })
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
            Some(&VersionState::Uploading {
                upload_id: id2,
                has_manifest: false
            })
        );
    }

    #[test]
    fn stream_category_is_component() {
        assert_eq!(ComponentAggregate::stream_category().as_str(), "component");
    }

    // ============================================================
    // TASKS/023 — Manifest recording (transactional publish prep)
    // ============================================================

    #[test]
    fn record_manifest_flips_has_manifest_flag() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id,
                has_manifest: false,
            })
        );

        ComponentAggregate::record_manifest(&mut root, id).unwrap();

        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id,
                has_manifest: true,
            })
        );
    }

    #[test]
    fn record_manifest_rejects_unknown_upload() {
        let mut root = new_root();
        let _id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        let bogus = Uuid::now_v7();
        let err = ComponentAggregate::record_manifest(&mut root, bogus);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not active"));
    }

    #[test]
    fn record_manifest_rejects_after_publish() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();

        // The upload is no longer in Uploading state — record_manifest must reject.
        let err = ComponentAggregate::record_manifest(&mut root, id);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not active"));
    }

    #[test]
    fn record_manifest_idempotent_on_repeat() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        ComponentAggregate::record_manifest(&mut root, id).unwrap();
        ComponentAggregate::record_manifest(&mut root, id).unwrap();

        // Flag is still set; no panic, no state corruption.
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id,
                has_manifest: true,
            })
        );
    }

    #[test]
    fn manifest_recorded_event_serde_roundtrip() {
        let event = ComponentEvent::ManifestRecorded {
            upload_id: Uuid::now_v7(),
        };
        let json = serde_json::to_value(&event).unwrap();
        let back: ComponentEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.event_type(), back.event_type());
        assert_eq!(event.event_type(), "component.manifest_recorded");
    }

    #[test]
    fn hydrate_replays_manifest_state() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::upload_file(&mut root, id, "deploy.yaml").unwrap();
        ComponentAggregate::record_manifest(&mut root, id).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "component-acme/widget".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        let replayed = AggregateRoot::<ComponentAggregate>::hydrate(
            "component-acme/widget".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(
            replayed.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id,
                has_manifest: true,
            })
        );
    }

    #[test]
    fn abort_upload_removes_inflight() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        ComponentAggregate::abort_upload(&mut root, id, "ctrl-c").unwrap();

        assert!(root.state.versions.get("1.0.0").is_none());
    }

    #[test]
    fn abort_upload_is_noop_for_unknown_id() {
        let mut root = new_root();
        let _id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        let pending_before = root.pending_count();

        let bogus = Uuid::now_v7();
        ComponentAggregate::abort_upload(&mut root, bogus, "no such upload").unwrap();

        // No new event recorded, no panic.
        assert_eq!(root.pending_count(), pending_before);
        assert!(matches!(
            root.state.versions.get("1.0.0"),
            Some(VersionState::Uploading { .. })
        ));
    }

    #[test]
    fn abort_upload_is_noop_after_publish() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();
        let pending_before = root.pending_count();

        ComponentAggregate::abort_upload(&mut root, id, "too late").unwrap();

        assert_eq!(root.pending_count(), pending_before);
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
    }

    #[test]
    fn abort_upload_then_rebegin_succeeds_without_supersede_event() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::abort_upload(&mut root, id1, "client crashed").unwrap();

        let pending_before = root.pending_count();
        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        // begin_upload only emits UploadStarted (no supersede UploadAborted) since
        // the previous upload was already aborted.
        assert_eq!(root.pending_count(), pending_before + 1);
        assert_ne!(id1, id2);
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id2,
                has_manifest: false,
            })
        );
    }

    #[test]
    fn publish_version_rejects_when_manifest_required_but_missing() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::upload_file(&mut root, id, "binary.bin").unwrap();

        // No record_manifest call. With require_manifest=true (= caller knows
        // this is a binary/external publish), this must reject.
        let err = ComponentAggregate::publish_version(&mut root, id, true);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("no manifest recorded"),
            "expected 'no manifest recorded' in: {msg}"
        );

        // Aggregate state is unchanged — still Uploading.
        assert!(matches!(
            root.state.versions.get("1.0.0"),
            Some(VersionState::Uploading {
                has_manifest: false,
                ..
            })
        ));
    }

    #[test]
    fn publish_version_succeeds_when_manifest_required_and_present() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::record_manifest(&mut root, id).unwrap();

        let version = ComponentAggregate::publish_version(&mut root, id, true).unwrap();
        assert_eq!(version, "1.0.0");
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
    }

    #[test]
    fn publish_version_succeeds_when_manifest_not_required_and_absent() {
        // CUE-only / Deno publish path: caller passes require_manifest=false,
        // and the absence of a manifest is fine.
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::upload_file(&mut root, id, "schema.cue").unwrap();

        let version = ComponentAggregate::publish_version(&mut root, id, false).unwrap();
        assert_eq!(version, "1.0.0");
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
    }

    #[test]
    fn publish_version_error_does_not_record_event() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        let pending_before = root.pending_count();

        let _ = ComponentAggregate::publish_version(&mut root, id, true);

        // No new event was recorded — the version remains in Uploading state
        // so a subsequent record_manifest + publish_version can succeed cleanly.
        assert_eq!(root.pending_count(), pending_before);
        ComponentAggregate::record_manifest(&mut root, id).unwrap();
        ComponentAggregate::publish_version(&mut root, id, true).unwrap();
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
    }

    // ============================================================
    // TASKS/025 — Unpublish
    // ============================================================

    #[test]
    fn unpublish_transitions_published_to_unpublished() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();

        let recorded =
            ComponentAggregate::unpublish_version(&mut root, "1.0.0", "user:alice", Some("oops"))
                .unwrap();
        assert!(recorded);
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Unpublished)
        );
    }

    #[test]
    fn unpublish_is_idempotent_on_already_unpublished() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();
        ComponentAggregate::unpublish_version(&mut root, "1.0.0", "user:alice", None).unwrap();

        let pending_before = root.pending_count();
        let recorded =
            ComponentAggregate::unpublish_version(&mut root, "1.0.0", "user:alice", None).unwrap();
        assert!(!recorded, "second unpublish must be a no-op");
        assert_eq!(root.pending_count(), pending_before);
    }

    #[test]
    fn unpublish_rejects_in_flight_uploads() {
        let mut root = new_root();
        let _id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        let err = ComponentAggregate::unpublish_version(&mut root, "1.0.0", "user:alice", None);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("in-flight"),
            "expected 'in-flight' hint in: {msg}"
        );
    }

    #[test]
    fn unpublish_rejects_unknown_version() {
        let mut root = new_root();
        let _id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        let err = ComponentAggregate::unpublish_version(&mut root, "9.9.9", "user:alice", None);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn republish_after_unpublish_is_allowed() {
        // Critical zombie-cleanup property: after admin unpublish, the
        // user can re-publish the same version cleanly. Without this,
        // unpublish would orphan the version number forever.
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id1, false).unwrap();
        ComponentAggregate::unpublish_version(&mut root, "1.0.0", "user:alice", None).unwrap();

        // begin_upload for the same version succeeds (Unpublished is treated
        // like absent).
        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        assert_ne!(id1, id2);
        ComponentAggregate::record_manifest(&mut root, id2).unwrap();
        ComponentAggregate::publish_version(&mut root, id2, true).unwrap();

        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Published)
        );
    }

    #[test]
    fn unpublish_event_replays_correctly() {
        let mut root = new_root();
        let id = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::publish_version(&mut root, id, false).unwrap();
        ComponentAggregate::unpublish_version(&mut root, "1.0.0", "user:alice", Some("ghost"))
            .unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "component-acme/widget".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: chrono::Utc::now(),
            })
            .collect();

        let replayed = AggregateRoot::<ComponentAggregate>::hydrate(
            "component-acme/widget".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(
            replayed.state.versions.get("1.0.0"),
            Some(&VersionState::Unpublished)
        );
    }

    #[test]
    fn unpublished_event_serde_roundtrip() {
        let event = ComponentEvent::VersionUnpublished {
            version: "1.0.0".into(),
            actor: "user:alice".into(),
            reason: Some("zombie cleanup".into()),
        };
        let json = serde_json::to_value(&event).unwrap();
        let back: ComponentEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.event_type(), back.event_type());
        assert_eq!(event.event_type(), "component.version_unpublished");
    }

    #[test]
    fn unpublished_event_without_reason() {
        let event = ComponentEvent::VersionUnpublished {
            version: "1.0.0".into(),
            actor: "user:alice".into(),
            reason: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        let back: ComponentEvent = serde_json::from_value(json).unwrap();
        if let ComponentEvent::VersionUnpublished { reason, .. } = back {
            assert!(reason.is_none());
        } else {
            panic!("variant changed");
        }
    }

    #[test]
    fn manifest_flag_resets_on_upload_supersede() {
        let mut root = new_root();
        let id1 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();
        ComponentAggregate::record_manifest(&mut root, id1).unwrap();

        // New upload for same version supersedes id1 — fresh has_manifest=false.
        let id2 = ComponentAggregate::begin_upload(&mut root, "acme", "widget", "1.0.0").unwrap();

        assert_ne!(id1, id2);
        assert_eq!(
            root.state.versions.get("1.0.0"),
            Some(&VersionState::Uploading {
                upload_id: id2,
                has_manifest: false,
            })
        );
    }
}
