use std::pin::Pin;

use anyhow::Context;
use futures::{Stream, StreamExt};
use forest_grpc_interface::{registry_service_server::RegistryService, *};
use uuid::Uuid;

use crate::{
    actor::Actor,
    grpc::authorize::{self, OrgRole},
    services::component_aggregate::{
        ComponentServiceState, ComponentVersion, FileStream,
    },
    state::State,
};

fn shape_to_proto(s: &str) -> ComponentShape {
    match s {
        "component" => ComponentShape::Component,
        "hybrid_component" => ComponentShape::Hybrid,
        "tool_binary" => ComponentShape::ToolBinary,
        "tool_external" => ComponentShape::ToolExternal,
        _ => ComponentShape::Unspecified,
    }
}

pub struct RegistryServer {
    pub state: State,
}

#[async_trait::async_trait]
impl RegistryService for RegistryServer {
    async fn get_components(
        &self,
        request: tonic::Request<GetComponentsRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentsResponse>, tonic::Status> {
        let _actor = authorize::extract_actor(&request)?;
        let _request = request.into_inner();
        Ok(tonic::Response::new(GetComponentsResponse {}))
    }

    #[tracing::instrument(skip(self), level = "trace")]
    async fn get_component(
        &self,
        request: tonic::Request<GetComponentRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentResponse>, tonic::Status> {
        tracing::info!("get component");
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &request.organisation, OrgRole::Member).await?;

        let component = self
            .state
            .component_service()
            .get_component(&request.name, &request.organisation)
            .await
            .inspect_err(|e| tracing::warn!("failed to get component: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(GetComponentResponse {
            component: component.map(|c| c.into()),
        }))
    }

    async fn get_component_version(
        &self,
        request: tonic::Request<GetComponentVersionRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentVersionResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member).await?;

        let component = self
            .state
            .component_service()
            .get_component_version(&req.name, &req.organisation, &req.version)
            .await
            .inspect_err(|e| tracing::warn!("failed to get component version: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(GetComponentVersionResponse {
            component: component.map(|c| c.into()),
        }))
    }

    async fn begin_upload(
        &self,
        request: tonic::Request<BeginUploadRequest>,
    ) -> std::result::Result<tonic::Response<BeginUploadResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &request.organisation, OrgRole::Member).await?;

        let upload_id = self
            .state
            .component_service()
            .begin_upload(&request.organisation, &request.name, &request.version)
            .await
            .inspect_err(|e| tracing::warn!("failed to begin upload: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(BeginUploadResponse {
            upload_context: upload_id.to_string(),
        }))
    }

    async fn upload_file(
        &self,
        request: tonic::Request<UploadFileRequest>,
    ) -> std::result::Result<tonic::Response<UploadFileResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();

        let upload_id: Uuid = request
            .upload_context
            .parse()
            .context("invalid upload_context UUID")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        authorize_upload(&self.state, &actor, upload_id).await?;

        self.state
            .component_service()
            .upload_file(upload_id, &request.file_path, &request.file_content)
            .await
            .inspect_err(|e| tracing::warn!("failed to upload file: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(UploadFileResponse {}))
    }

    async fn commit_upload(
        &self,
        request: tonic::Request<CommitUploadRequest>,
    ) -> std::result::Result<tonic::Response<CommitUploadResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();

        let upload_id: Uuid = request
            .upload_context
            .parse()
            .context("invalid upload_context UUID")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        authorize_upload(&self.state, &actor, upload_id).await?;

        self.state
            .component_service()
            .commit_upload(upload_id)
            .await
            .inspect_err(|e| tracing::warn!("failed to commit upload: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(CommitUploadResponse {}))
    }

    type GetComponentFilesStream = Pin<
        Box<
            dyn Stream<Item = std::result::Result<GetComponentFilesResponse, tonic::Status>> + Send,
        >,
    >;
    async fn get_component_files(
        &self,
        request: tonic::Request<GetComponentFilesRequest>,
    ) -> std::result::Result<tonic::Response<Self::GetComponentFilesStream>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();

        let component_id: Uuid = request
            .component_id
            .parse()
            .context("failed to parse uuid")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        authorize_component(&self.state, &actor, component_id).await?;

        let mut stream = FileStream::new();
        let take_stream = stream.take_stream();

        let service = self.state.component_service();
        tokio::spawn(async move {
            if let Err(e) = service.get_files(component_id, stream).await {
                tracing::error!("failed to send files: {e:#}");
            }
        });

        Ok(tonic::Response::new(take_stream))
    }

    // --- v2: binary component RPCs ---

    async fn upload_binary(
        &self,
        request: tonic::Request<tonic::Streaming<UploadBinaryRequest>>,
    ) -> std::result::Result<tonic::Response<UploadBinaryResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let mut stream = request.into_inner();

        // First message must be metadata
        let first = stream
            .next()
            .await
            .ok_or_else(|| tonic::Status::invalid_argument("empty stream"))?
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let metadata = match first.msg {
            Some(upload_binary_request::Msg::Metadata(m)) => m,
            _ => {
                return Err(tonic::Status::invalid_argument(
                    "first message must be metadata",
                ))
            }
        };

        let upload_id: Uuid = metadata
            .upload_context
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid upload_context UUID"))?;

        authorize_upload(&self.state, &actor, upload_id).await?;

        // Collect binary chunks with size limit
        let mut binary_content = Vec::new();
        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| tonic::Status::internal(e.to_string()))?;
            match msg.msg {
                Some(upload_binary_request::Msg::Chunk(chunk)) => {
                    binary_content.extend_from_slice(&chunk);
                    if binary_content.len() > MAX_BINARY_UPLOAD_SIZE {
                        return Err(tonic::Status::invalid_argument(format!(
                            "binary exceeds maximum size of {} bytes",
                            MAX_BINARY_UPLOAD_SIZE
                        )));
                    }
                }
                _ => {
                    return Err(tonic::Status::invalid_argument(
                        "expected chunk after metadata",
                    ))
                }
            }
        }

        // Verify SHA-256
        use sha2::{Digest, Sha256};
        let actual_sha256 = hex::encode(Sha256::digest(&binary_content));
        if actual_sha256 != metadata.sha256 {
            return Err(tonic::Status::invalid_argument(format!(
                "sha256 mismatch: expected {}, got {}",
                metadata.sha256, actual_sha256
            )));
        }

        let size_bytes = self
            .state
            .component_service()
            .upload_binary(
                upload_id,
                &metadata.os,
                &metadata.arch,
                &metadata.sha256,
                &binary_content,
            )
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(UploadBinaryResponse { size_bytes }))
    }

    type DownloadBinaryStream = Pin<
        Box<dyn Stream<Item = std::result::Result<DownloadBinaryResponse, tonic::Status>> + Send>,
    >;

    async fn download_binary(
        &self,
        request: tonic::Request<DownloadBinaryRequest>,
    ) -> std::result::Result<tonic::Response<Self::DownloadBinaryStream>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member).await?;

        let binary_content = self
            .state
            .component_service()
            .download_binary(&req.organisation, &req.name, &req.version, &req.os, &req.arch)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        // Stream in 1MB chunks
        let chunk_size = 1024 * 1024;
        let chunks: Vec<_> = binary_content
            .chunks(chunk_size)
            .map(|chunk| {
                Ok(DownloadBinaryResponse {
                    chunk: chunk.to_vec(),
                })
            })
            .collect();

        let stream = futures::stream::iter(chunks);
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    async fn publish_manifest(
        &self,
        request: tonic::Request<PublishManifestRequest>,
    ) -> std::result::Result<tonic::Response<PublishManifestResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let upload_id: Uuid = req
            .upload_context
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid upload_context UUID"))?;

        authorize_upload(&self.state, &actor, upload_id).await?;

        self.state
            .component_service()
            .publish_manifest(upload_id, &req.manifest_json)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(PublishManifestResponse {}))
    }

    async fn get_component_manifest(
        &self,
        request: tonic::Request<GetComponentManifestRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentManifestResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member).await?;

        let manifest_json = self
            .state
            .component_service()
            .get_manifest(&req.organisation, &req.name, &req.version)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?
            .ok_or_else(|| {
                tonic::Status::not_found(format!(
                    "manifest not found for {}/{}@{}",
                    req.organisation, req.name, req.version
                ))
            })?;

        Ok(tonic::Response::new(GetComponentManifestResponse {
            manifest_json,
        }))
    }

    async fn list_component_versions(
        &self,
        request: tonic::Request<ListComponentVersionsRequest>,
    ) -> std::result::Result<tonic::Response<ListComponentVersionsResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member).await?;

        let versions = self
            .state
            .component_service()
            .list_versions(&req.organisation, &req.name)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(ListComponentVersionsResponse {
            versions: versions
                .into_iter()
                .map(|v| forest_grpc_interface::ComponentVersionInfo {
                    version: v.version,
                    protocol_version: v.protocol_version,
                    kind: v.kind,
                    platforms: v.platforms,
                })
                .collect(),
        }))
    }

    // --- Global-tools (TASKS/018-global-tools.md §1a.2c) ---

    type ListOrgToolsStream = Pin<
        Box<dyn Stream<Item = std::result::Result<OrgToolEntry, tonic::Status>> + Send>,
    >;

    async fn list_org_tools(
        &self,
        request: tonic::Request<ListOrgToolsRequest>,
    ) -> std::result::Result<tonic::Response<Self::ListOrgToolsStream>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            OrgRole::Member,
        )
        .await?;

        let rows = self
            .state
            .component_service()
            .list_org_tools(&req.organisation)
            .await
            .inspect_err(|e| tracing::warn!("list_org_tools failed: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let stream = futures::stream::iter(rows.into_iter().map(|row| {
            Ok(OrgToolEntry {
                organisation: row.organisation,
                name: row.name,
                latest_version: row.latest_version,
                shape: shape_to_proto(&row.shape) as i32,
                upstream_host: row.upstream_host.unwrap_or_default(),
                tool: row.tool.map(|t| ToolFacet {
                    name: t.name,
                    argv_passthrough: t.argv_passthrough,
                    description: t.description.unwrap_or_default(),
                }),
            })
        }));
        Ok(tonic::Response::new(Box::pin(stream)))
    }

    // --- Registry UI / discovery ---

    async fn search_components(
        &self,
        request: tonic::Request<SearchComponentsRequest>,
    ) -> std::result::Result<tonic::Response<SearchComponentsResponse>, tonic::Status> {
        let actor = authorize::try_extract_actor(&request);
        let req = request.into_inner();

        // Resolve visibility scope based on caller:
        // - Anonymous: public projects only
        // - Service account: all components (cross-org infra access)
        // - User: public projects + private projects from their orgs
        // - App: public projects + their org's private projects
        let member_orgs = match &actor {
            None => vec![], // anonymous
            Some(Actor::ServiceAccount { .. }) => vec![], // sees all via public_only=false
            Some(Actor::User { user_id }) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT o.name FROM organisations o
                     JOIN organisation_members om ON om.organisation_id = o.id
                     WHERE om.user_id = $1",
                )
                .bind(user_id)
                .fetch_all(&self.state.db)
                .await
                .unwrap_or_default()
            }
            Some(Actor::App { organisation_id, .. }) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT name FROM organisations WHERE id = $1",
                )
                .bind(organisation_id)
                .fetch_all(&self.state.db)
                .await
                .unwrap_or_default()
            }
        };
        let see_all = matches!(&actor, Some(Actor::ServiceAccount { .. }));

        let page = req.page.max(0) as i64;
        let page_size = req.page_size.clamp(1, 100) as i64;
        let offset = page * page_size;

        let (rows, total_count) = self
            .state
            .component_service()
            .search_components(&req.query, &req.organisation, page_size, offset, see_all, &member_orgs)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(SearchComponentsResponse {
            components: rows,
            total_count,
        }))
    }

    async fn get_component_detail(
        &self,
        request: tonic::Request<GetComponentDetailRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentDetailResponse>, tonic::Status> {
        let actor = authorize::try_extract_actor(&request);
        let req = request.into_inner();

        // Check access: authenticated users need org membership for private components.
        // Unauthenticated users can only see public components.
        let is_authenticated = actor.is_some();
        if let Some(ref actor) = actor {
            // Authenticated: enforce org membership (service accounts bypass this)
            authorize::require_org_access(
                &self.state.db,
                actor,
                &req.organisation,
                OrgRole::Member,
            )
            .await?;
        }

        let detail = self
            .state
            .component_service()
            .get_component_detail(&req.organisation, &req.name)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?
            .ok_or_else(|| {
                tonic::Status::not_found(format!(
                    "component not found: {}/{}",
                    req.organisation, req.name
                ))
            })?;

        // Unauthenticated users can only see components from public projects.
        if !is_authenticated {
            let is_public: Option<bool> = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                    SELECT 1 FROM projects p
                    WHERE p.organisation = $1 AND p.project = $2 AND p.visibility = 'public'
                )",
            )
            .bind(&req.organisation)
            .bind(&req.name)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
            let is_public = is_public.unwrap_or(false);

            if !is_public {
                return Err(tonic::Status::not_found(format!(
                    "component not found: {}/{}",
                    req.organisation, req.name
                )));
            }
        }

        Ok(tonic::Response::new(detail))
    }
}

/// Maximum binary upload size: 500 MB.
const MAX_BINARY_UPLOAD_SIZE: usize = 500 * 1024 * 1024;

/// Look up the owning organisation for an upload_context UUID, then check org access.
async fn authorize_upload(
    state: &State,
    actor: &crate::actor::Actor,
    upload_id: Uuid,
) -> Result<(), tonic::Status> {
    let org: String = sqlx::query_scalar(
        "SELECT organisation FROM component_staging WHERE id = $1 AND status = 'staged'",
    )
    .bind(upload_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tonic::Status::internal(format!("failed to resolve upload: {e}")))?
    .ok_or_else(|| tonic::Status::not_found("upload not found or already committed"))?;

    authorize::require_org_access(&state.db, actor, &org, OrgRole::Member).await?;
    Ok(())
}

/// Look up the owning organisation for a component UUID, then check org access.
async fn authorize_component(
    state: &State,
    actor: &crate::actor::Actor,
    component_id: Uuid,
) -> Result<(), tonic::Status> {
    let org: String = sqlx::query_scalar(
        "SELECT organisation FROM components WHERE id = $1",
    )
    .bind(component_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| tonic::Status::internal(format!("failed to resolve component: {e}")))?
    .ok_or_else(|| tonic::Status::not_found("component not found"))?;

    authorize::require_org_access(&state.db, actor, &org, OrgRole::Member).await?;
    Ok(())
}

impl From<ComponentVersion> for Component {
    fn from(value: ComponentVersion) -> Self {
        Self {
            id: value.id,
            version: value.version,
        }
    }
}
