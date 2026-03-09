use std::pin::Pin;

use anyhow::Context;
use forest_event_store::EventStore;
use futures::{SinkExt, Stream};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domains::component::{self, ComponentAggregate};

// ============================================================
// Read-model types
// ============================================================

pub struct ComponentVersion {
    pub id: String,
    pub name: String,
    pub organisation: String,
    pub version: String,
}

// ============================================================
// Service — orchestrates aggregate + projections
// ============================================================

/// In-flight upload metadata resolved from the staging projection.
struct UploadInfo {
    organisation: String,
    name: String,
}

#[derive(Clone)]
pub struct ComponentService {
    event_store: EventStore,
    db: PgPool,
}

impl ComponentService {
    pub fn new(event_store: EventStore, db: PgPool) -> Self {
        Self { event_store, db }
    }

    // ----------------------------------------------------------
    // Commands
    // ----------------------------------------------------------

    /// Begin a component version upload. Returns the upload_id (UUID).
    ///
    /// Projections updated atomically:
    /// - `component_staging` row inserted (status='staged')
    pub async fn begin_upload(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> anyhow::Result<Uuid> {
        let key = component::stream_key(organisation, name);
        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        let upload_id = ComponentAggregate::begin_upload(&mut root, organisation, name, version)?;

        let org = organisation.to_string();
        let name_owned = name.to_string();
        let version_owned = version.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO component_staging (id, name, organisation, version, status)
                         VALUES ($1, $2, $3, $4, 'staged')
                         ON CONFLICT (name, organisation, version)
                         DO UPDATE SET id = $1, status = 'staged', updated = now()",
                    )
                    .bind(upload_id)
                    .bind(&name_owned)
                    .bind(&org)
                    .bind(&version_owned)
                    .execute(&mut **tx)
                    .await
                    .context("insert staging projection")?;
                    Ok(())
                })
            })
            .await?;

        Ok(upload_id)
    }

    /// Upload a file for an in-flight upload.
    ///
    /// Resolves org/name from the staging projection, then:
    /// - Records FileUploaded event
    /// - Inserts into `component_files` projection atomically
    pub async fn upload_file(
        &self,
        upload_id: Uuid,
        file_path: &str,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        let info = self.resolve_upload(upload_id).await?;
        let key = component::stream_key(&info.organisation, &info.name);

        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        ComponentAggregate::upload_file(&mut root, upload_id, file_path)?;

        let file_path_owned = file_path.to_string();
        let file_content_owned = file_content.to_vec();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO component_files (component_id, file_path, file_content)
                         VALUES ($1, $2, $3)",
                    )
                    .bind(upload_id)
                    .bind(&file_path_owned)
                    .bind(&file_content_owned)
                    .execute(&mut **tx)
                    .await
                    .context("insert component file")?;
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    /// Commit (publish) an upload.
    ///
    /// Projections updated atomically:
    /// - `components` row upserted
    /// - `component_staging` status set to 'committed'
    pub async fn commit_upload(&self, upload_id: Uuid) -> anyhow::Result<()> {
        let info = self.resolve_upload(upload_id).await?;
        let key = component::stream_key(&info.organisation, &info.name);

        let mut root = self
            .event_store
            .load_or_default::<ComponentAggregate>(&key)
            .await?;

        let version = ComponentAggregate::publish_version(&mut root, upload_id)?;

        let org = info.organisation;
        let name = info.name;

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO components (id, name, organisation, version)
                         VALUES ($1, $2, $3, $4)
                         ON CONFLICT (name, organisation, version) DO NOTHING",
                    )
                    .bind(upload_id)
                    .bind(&name)
                    .bind(&org)
                    .bind(&version)
                    .execute(&mut **tx)
                    .await
                    .context("upsert component projection")?;

                    sqlx::query(
                        "UPDATE component_staging SET status = 'committed', updated = now()
                         WHERE id = $1 AND status = 'staged'",
                    )
                    .bind(upload_id)
                    .execute(&mut **tx)
                    .await
                    .context("update staging status")?;

                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    // ----------------------------------------------------------
    // Queries (read from projections)
    // ----------------------------------------------------------

    /// Get the latest version of a component.
    pub async fn get_component(
        &self,
        name: &str,
        organisation: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let row = sqlx::query(
            "SELECT id, name, organisation, version
             FROM components
             WHERE name = $1 AND organisation = $2
             ORDER BY version DESC
             LIMIT 1",
        )
        .bind(name)
        .bind(organisation)
        .fetch_optional(&self.db)
        .await
        .context("get component")?;

        Ok(row.map(|r| ComponentVersion {
            id: r.get::<Uuid, _>("id").to_string(),
            name: r.get("name"),
            organisation: r.get("organisation"),
            version: r.get("version"),
        }))
    }

    /// Get a specific component version.
    pub async fn get_component_version(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let row = sqlx::query(
            "SELECT id, name, organisation, version
             FROM components
             WHERE name = $1 AND organisation = $2 AND version = $3",
        )
        .bind(name)
        .bind(organisation)
        .bind(version)
        .fetch_optional(&self.db)
        .await
        .context("get component version")?;

        Ok(row.map(|r| ComponentVersion {
            id: r.get::<Uuid, _>("id").to_string(),
            name: r.get("name"),
            organisation: r.get("organisation"),
            version: r.get("version"),
        }))
    }

    /// Stream files for a published component.
    pub async fn get_files(
        &self,
        component_id: Uuid,
        file_stream: FileStream,
    ) -> anyhow::Result<()> {
        let mut page: i64 = 0;
        loop {
            let row = sqlx::query(
                "SELECT file_path, file_content
                 FROM component_files
                 WHERE component_id = $1
                 ORDER BY file_path ASC
                 LIMIT 1 OFFSET $2",
            )
            .bind(component_id)
            .bind(page)
            .fetch_optional(&self.db)
            .await;

            match row {
                Ok(Some(r)) => {
                    let path: String = r.get("file_path");
                    let content: Vec<u8> = r.get("file_content");
                    if let Err(e) = file_stream.push_file(&path, &content).await {
                        file_stream.push_err(e).await?;
                        return Ok(());
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    file_stream.push_err(e.into()).await?;
                    return Ok(());
                }
            }

            page += 1;
        }

        file_stream.push_done().await?;
        Ok(())
    }

    // ----------------------------------------------------------
    // Internal helpers
    // ----------------------------------------------------------

    /// Resolve upload_id → (organisation, name) from staging projection.
    async fn resolve_upload(&self, upload_id: Uuid) -> anyhow::Result<UploadInfo> {
        let row = sqlx::query(
            "SELECT organisation, name FROM component_staging
             WHERE id = $1 AND status = 'staged'",
        )
        .bind(upload_id)
        .fetch_optional(&self.db)
        .await
        .context("resolve upload")?
        .with_context(|| format!("upload {} not found or already committed", upload_id))?;

        Ok(UploadInfo {
            organisation: row.get("organisation"),
            name: row.get("name"),
        })
    }
}

// ============================================================
// FileStream — gRPC streaming helper
// ============================================================

pub struct FileStream {
    rx: Option<
        futures::channel::mpsc::Receiver<
            std::result::Result<forest_grpc_interface::GetComponentFilesResponse, tonic::Status>,
        >,
    >,
    tx: futures::channel::mpsc::Sender<
        std::result::Result<forest_grpc_interface::GetComponentFilesResponse, tonic::Status>,
    >,
}

impl Default for FileStream {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStream {
    pub fn new() -> Self {
        let (tx, rx) = futures::channel::mpsc::channel(10);
        Self { tx, rx: Some(rx) }
    }

    pub fn take_stream(
        &mut self,
    ) -> Pin<
        Box<
            dyn Stream<
                    Item = std::result::Result<
                        forest_grpc_interface::GetComponentFilesResponse,
                        tonic::Status,
                    >,
                > + Send,
        >,
    > {
        Box::pin(self.rx.take().expect("to only take stream once"))
    }

    pub async fn push_err(&self, error: anyhow::Error) -> anyhow::Result<()> {
        self.tx
            .clone()
            .send(Err(tonic::Status::internal(error.to_string())))
            .await?;
        Ok(())
    }

    pub async fn push_file(&self, file_path: &str, file_content: &[u8]) -> anyhow::Result<()> {
        self.tx
            .clone()
            .send(Ok(forest_grpc_interface::GetComponentFilesResponse {
                msg: Some(
                    forest_grpc_interface::get_component_files_response::Msg::ComponentFile(
                        forest_grpc_interface::ComponentFile {
                            file_path: file_path.into(),
                            file_content: file_content.into(),
                        },
                    ),
                ),
            }))
            .await?;
        Ok(())
    }

    pub async fn push_done(mut self) -> anyhow::Result<()> {
        self.tx
            .send(Ok(forest_grpc_interface::GetComponentFilesResponse {
                msg: Some(
                    forest_grpc_interface::get_component_files_response::Msg::Done(
                        forest_grpc_interface::Done {},
                    ),
                ),
            }))
            .await?;
        self.tx.close_channel();
        Ok(())
    }
}

// ============================================================
// State integration
// ============================================================

pub trait ComponentServiceState {
    fn component_service(&self) -> ComponentService;
}

impl ComponentServiceState for crate::state::State {
    fn component_service(&self) -> ComponentService {
        ComponentService::new(self.event_store.clone(), self.db.clone())
    }
}
