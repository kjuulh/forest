use std::pin::Pin;

use anyhow::Context;
use forest_grpc_interface::{registry_service_server::RegistryService, *};
use futures::{Stream, StreamExt};
use uuid::Uuid;

use crate::{
    actor::Actor,
    grpc::authorize::{self, OrgRole},
    services::{
        component_aggregate::{
            BinaryArtifact, BinaryId, BinaryRange, ComponentServiceState, ComponentVersion,
            DownloadBinaryError, FileStream,
        },
        oauth_apps::SCOPE_COMPONENTS_PUBLISH,
    },
    state::State,
};

const DOWNLOAD_CHUNK_SIZE: usize = 1024 * 1024;

fn chunked_download(
    bytes: crate::object_store::ByteStream,
    chunk_size: usize,
    start: u64,
    artifact: BinaryArtifact,
) -> impl Stream<Item = std::result::Result<DownloadBinaryResponse, tonic::Status>> + Send {
    struct Cursor {
        bytes: crate::object_store::ByteStream,
        buffer: Vec<u8>,
        offset: u64,
        artifact: Option<BinaryArtifact>,
        finished: bool,
    }

    impl Cursor {
        fn message(&mut self, chunk: Vec<u8>) -> DownloadBinaryResponse {
            let offset = self.offset;
            self.offset += chunk.len() as u64;
            let (total_size, sha256) = match self.artifact.take() {
                Some(a) => (a.size_bytes, a.sha256),
                None => (0, String::new()),
            };
            DownloadBinaryResponse {
                chunk,
                offset,
                total_size,
                sha256,
            }
        }
    }

    futures::stream::unfold(
        Cursor {
            bytes,
            buffer: Vec::with_capacity(chunk_size),
            offset: start,
            artifact: Some(artifact),
            finished: false,
        },
        move |mut cursor| async move {
            if cursor.finished {
                return None;
            }
            loop {
                let first = cursor.artifact.is_some();
                if cursor.buffer.len() >= chunk_size || (first && !cursor.buffer.is_empty()) {
                    let cut = cursor.buffer.len().min(chunk_size);
                    let remainder = cursor.buffer.split_off(cut);
                    let chunk = std::mem::replace(&mut cursor.buffer, remainder);
                    let msg = cursor.message(chunk);
                    return Some((Ok(msg), cursor));
                }
                match cursor.bytes.next().await {
                    Some(Ok(part)) => cursor.buffer.extend_from_slice(&part),
                    Some(Err(e)) => {
                        cursor.finished = true;
                        return Some((Err(tonic::Status::internal(format!("{e:#}"))), cursor));
                    }
                    None => {
                        cursor.finished = true;
                        if cursor.buffer.is_empty() {
                            return None;
                        }
                        let chunk = std::mem::take(&mut cursor.buffer);
                        let msg = cursor.message(chunk);
                        return Some((Ok(msg), cursor));
                    }
                }
            }
        },
    )
}

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
        authorize::require_component_read(
            &self.state.db,
            &actor,
            &request.organisation,
            &request.name,
        )
        .await?;

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
        authorize::require_component_read(&self.state.db, &actor, &req.organisation, &req.name)
            .await?;

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
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &request.organisation,
            OrgRole::Member,
        )
        .await?;
        require_component_publish_scope(&actor)?;

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

    async fn unpublish_version(
        &self,
        request: tonic::Request<UnpublishVersionRequest>,
    ) -> std::result::Result<tonic::Response<UnpublishVersionResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &request.organisation,
            OrgRole::Member,
        )
        .await?;
        require_component_publish_scope(&actor)?;

        let reason = if request.reason.is_empty() {
            None
        } else {
            Some(request.reason.as_str())
        };

        let unpublished = self
            .state
            .component_service()
            .unpublish_version(
                &request.organisation,
                &request.name,
                &request.version,
                &format!("{:?}", actor),
                reason,
            )
            .await
            .inspect_err(|e| tracing::warn!("failed to unpublish version: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(UnpublishVersionResponse {
            unpublished,
        }))
    }

    async fn abort_upload(
        &self,
        request: tonic::Request<AbortUploadRequest>,
    ) -> std::result::Result<tonic::Response<AbortUploadResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let request = request.into_inner();

        let upload_id: Uuid = request
            .upload_context
            .parse()
            .context("invalid upload_context UUID")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        if let Ok(()) = authorize_upload(&self.state, &actor, upload_id).await {
            self.state
                .component_service()
                .abort_upload(upload_id, &request.reason)
                .await
                .inspect_err(|e| tracing::warn!("failed to abort upload: {e:#}"))
                .map_err(|e| tonic::Status::internal(e.to_string()))?;
        }

        Ok(tonic::Response::new(AbortUploadResponse {}))
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
        authorize_component_files(&self.state, &actor, component_id).await?;

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

    async fn upload_binary(
        &self,
        request: tonic::Request<tonic::Streaming<UploadBinaryRequest>>,
    ) -> std::result::Result<tonic::Response<UploadBinaryResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let mut stream = request.into_inner();

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
                ));
            }
        };

        let upload_id: Uuid = metadata
            .upload_context
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid upload_context UUID"))?;

        authorize_upload(&self.state, &actor, upload_id).await?;

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
                    ));
                }
            }
        }

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
        authorize::require_component_read(&self.state.db, &actor, &req.organisation, &req.name)
            .await?;

        let range = match (req.offset, req.length) {
            (0, 0) => BinaryRange::Whole,
            (_, 0) => {
                return Err(tonic::Status::invalid_argument(
                    "offset requires a non-zero length",
                ));
            }
            (offset, length) => BinaryRange::Bytes { offset, length },
        };
        let expected_sha256 =
            (!req.expected_sha256.is_empty()).then_some(req.expected_sha256.as_str());

        let id = BinaryId {
            organisation: &req.organisation,
            name: &req.name,
            version: &req.version,
            os: &req.os,
            arch: &req.arch,
        };
        let (artifact, byte_stream) = self
            .state
            .component_service()
            .download_binary_stream(&id, range, expected_sha256)
            .await
            .map_err(|e| match e {
                DownloadBinaryError::NotFound(_) => tonic::Status::not_found(e.to_string()),
                DownloadBinaryError::DigestChanged { .. } => {
                    tonic::Status::failed_precondition(e.to_string())
                }
                DownloadBinaryError::OutOfRange { .. } => {
                    tonic::Status::out_of_range(e.to_string())
                }
                DownloadBinaryError::Other(e) => tonic::Status::internal(format!("{e:#}")),
            })?;

        let start = match range {
            BinaryRange::Whole => 0,
            BinaryRange::Bytes { offset, .. } => offset,
        };
        Ok(tonic::Response::new(Box::pin(chunked_download(
            byte_stream,
            DOWNLOAD_CHUNK_SIZE,
            start,
            artifact,
        ))))
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
        authorize::require_component_read(&self.state.db, &actor, &req.organisation, &req.name)
            .await?;

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
        authorize::require_component_read(&self.state.db, &actor, &req.organisation, &req.name)
            .await?;

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
                    created_at: v.created_at,
                })
                .collect(),
        }))
    }

    type ListOrgToolsStream =
        Pin<Box<dyn Stream<Item = std::result::Result<OrgToolEntry, tonic::Status>> + Send>>;

    async fn list_org_tools(
        &self,
        request: tonic::Request<ListOrgToolsRequest>,
    ) -> std::result::Result<tonic::Response<Self::ListOrgToolsStream>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_read(&self.state.db, &actor, &req.organisation).await?;

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

    async fn search_components(
        &self,
        request: tonic::Request<SearchComponentsRequest>,
    ) -> std::result::Result<tonic::Response<SearchComponentsResponse>, tonic::Status> {
        let actor = authorize::try_extract_actor(&request);
        let req = request.into_inner();

        let member_orgs = match &actor {
            None => vec![],
            Some(Actor::ServiceAccount { .. }) => vec![],
            Some(Actor::User { user_id }) => sqlx::query_scalar::<_, String>(
                "SELECT o.name FROM organisations o
                     JOIN organisation_members om ON om.organisation_id = o.id
                     WHERE om.user_id = $1",
            )
            .bind(user_id)
            .fetch_all(&self.state.db)
            .await
            .unwrap_or_default(),
            Some(Actor::App {
                organisation_id, ..
            }) => sqlx::query_scalar::<_, String>("SELECT name FROM organisations WHERE id = $1")
                .bind(organisation_id)
                .fetch_all(&self.state.db)
                .await
                .unwrap_or_default(),
        };
        let see_all = matches!(&actor, Some(Actor::ServiceAccount { .. }));

        let page = req.page.max(0) as i64;
        let page_size = req.page_size.clamp(1, 100) as i64;
        let offset = page * page_size;

        let (rows, total_count) = self
            .state
            .component_service()
            .search_components(
                &req.query,
                &req.organisation,
                page_size,
                offset,
                see_all,
                &member_orgs,
            )
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

        let is_authenticated = actor.is_some();
        if let Some(ref actor) = actor {
            authorize::require_component_read(&self.state.db, actor, &req.organisation, &req.name)
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

        if !is_authenticated {
            let is_public =
                authorize::component_is_public(&self.state.db, &req.organisation, &req.name)
                    .await?;

            if !is_public {
                return Err(tonic::Status::not_found(format!(
                    "component not found: {}/{}",
                    req.organisation, req.name
                )));
            }
        }

        Ok(tonic::Response::new(detail))
    }

    async fn search_public_components(
        &self,
        request: tonic::Request<SearchPublicComponentsRequest>,
    ) -> std::result::Result<tonic::Response<SearchPublicComponentsResponse>, tonic::Status> {
        let req = request.into_inner();

        let page = req.page.max(0) as i64;
        let page_size = req.page_size.clamp(1, 100) as i64;
        let offset = page * page_size;

        let (rows, total_count) = self
            .state
            .component_service()
            .search_components(&req.query, &req.organisation, page_size, offset, false, &[])
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(SearchPublicComponentsResponse {
            components: rows,
            total_count,
        }))
    }

    async fn get_public_component_detail(
        &self,
        request: tonic::Request<GetPublicComponentDetailRequest>,
    ) -> std::result::Result<tonic::Response<GetPublicComponentDetailResponse>, tonic::Status> {
        let req = request.into_inner();

        let is_public =
            authorize::component_is_public(&self.state.db, &req.organisation, &req.name).await?;
        if !is_public {
            return Err(tonic::Status::not_found(format!(
                "component not found: {}/{}",
                req.organisation, req.name
            )));
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

        Ok(tonic::Response::new(GetPublicComponentDetailResponse {
            summary: detail.summary,
            versions: detail.versions,
            readme: detail.readme,
            manifest_json: detail.manifest_json,
            owners: detail.owners,
        }))
    }

    async fn get_public_component_manifest(
        &self,
        request: tonic::Request<GetPublicComponentManifestRequest>,
    ) -> std::result::Result<tonic::Response<GetPublicComponentManifestResponse>, tonic::Status>
    {
        let req = request.into_inner();

        let is_public =
            authorize::component_is_public(&self.state.db, &req.organisation, &req.name).await?;
        if !is_public {
            return Err(tonic::Status::not_found(format!(
                "manifest not found for {}/{}@{}",
                req.organisation, req.name, req.version
            )));
        }

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

        Ok(tonic::Response::new(GetPublicComponentManifestResponse {
            manifest_json,
        }))
    }
}

const MAX_BINARY_UPLOAD_SIZE: usize = 500 * 1024 * 1024;

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
    require_component_publish_scope(actor)?;
    Ok(())
}

fn require_component_publish_scope(actor: &Actor) -> Result<(), tonic::Status> {
    if let Actor::App {
        scopes: Some(scopes),
        ..
    } = actor
        && !scopes.iter().any(|scope| scope == SCOPE_COMPONENTS_PUBLISH)
    {
        return Err(tonic::Status::permission_denied(format!(
            "token lacks the {SCOPE_COMPONENTS_PUBLISH} scope"
        )));
    }
    Ok(())
}

async fn authorize_component_files(
    state: &State,
    actor: &crate::actor::Actor,
    component_id: Uuid,
) -> Result<(), tonic::Status> {
    let (org, name): (String, String) =
        sqlx::query_as("SELECT organisation, name FROM components WHERE id = $1")
            .bind(component_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| tonic::Status::internal(format!("failed to resolve component: {e}")))?
            .ok_or_else(|| tonic::Status::not_found("component not found"))?;

    authorize::require_component_read(&state.db, actor, &org, &name).await
}

impl From<ComponentVersion> for Component {
    fn from(value: ComponentVersion) -> Self {
        Self {
            id: value.id,
            version: value.version,
        }
    }
}

#[cfg(test)]
mod component_publish_scope_tests {
    use super::{Actor, SCOPE_COMPONENTS_PUBLISH, require_component_publish_scope};
    use uuid::Uuid;

    fn app(scopes: Option<Vec<&str>>) -> Actor {
        Actor::App {
            app_id: Uuid::now_v7(),
            organisation_id: Uuid::now_v7(),
            scopes: scopes.map(|s| s.into_iter().map(str::to_owned).collect()),
        }
    }

    #[test]
    fn a_client_token_without_the_publish_grant_cannot_change_the_registry() {
        let err = require_component_publish_scope(&app(Some(vec!["directory:read"]))).unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn a_client_token_with_no_scopes_at_all_cannot_change_the_registry() {
        assert!(require_component_publish_scope(&app(Some(vec![]))).is_err());
    }

    #[test]
    fn a_client_token_with_the_publish_grant_can() {
        assert!(
            require_component_publish_scope(&app(Some(vec![
                "directory:read",
                SCOPE_COMPONENTS_PUBLISH
            ])))
            .is_ok()
        );
    }

    #[test]
    fn a_legacy_app_token_keeps_its_access() {
        assert!(require_component_publish_scope(&app(None)).is_ok());
    }
}

#[cfg(test)]
mod download_chunking_tests {
    use super::*;

    fn stream_of(parts: Vec<Vec<u8>>) -> crate::object_store::ByteStream {
        Box::pin(futures::stream::iter(
            parts.into_iter().map(|p| Ok(bytes::Bytes::from(p))),
        ))
    }

    fn failing_stream(parts: Vec<Vec<u8>>, error: &'static str) -> crate::object_store::ByteStream {
        let ok = parts
            .into_iter()
            .map(|p| Ok(bytes::Bytes::from(p)))
            .collect::<Vec<_>>();
        Box::pin(
            futures::stream::iter(ok).chain(futures::stream::once(async move {
                Err(anyhow::anyhow!(error))
            })),
        )
    }

    fn artifact() -> BinaryArtifact {
        BinaryArtifact {
            sha256: "ab".repeat(32),
            size_bytes: 4242,
        }
    }

    fn chunked(
        bytes: crate::object_store::ByteStream,
        chunk_size: usize,
    ) -> impl Stream<Item = std::result::Result<DownloadBinaryResponse, tonic::Status>> {
        chunked_download(bytes, chunk_size, 0, artifact())
    }

    async fn collect(
        stream: impl Stream<Item = std::result::Result<DownloadBinaryResponse, tonic::Status>>,
    ) -> std::result::Result<Vec<Vec<u8>>, tonic::Status> {
        let mut out = Vec::new();
        let mut stream = Box::pin(stream);
        while let Some(item) = stream.next().await {
            out.push(item?.chunk);
        }
        Ok(out)
    }

    #[tokio::test]
    async fn coalesces_small_parts_into_full_chunks_after_the_first() {
        let parts = (0..10).map(|i| vec![i as u8; 100]).collect();
        let chunks = collect(chunked(stream_of(parts), 256)).await.unwrap();
        let sizes: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        assert_eq!(sizes, vec![100, 256, 256, 256, 132]);
        assert_eq!(sizes.iter().sum::<usize>(), 1000);
    }

    #[tokio::test]
    async fn splits_parts_larger_than_the_chunk_size() {
        let chunks = collect(chunked(stream_of(vec![vec![7u8; 1000]]), 256))
            .await
            .unwrap();
        let sizes: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        assert_eq!(sizes, vec![256, 256, 256, 232]);
    }

    #[tokio::test]
    async fn reassembles_to_exactly_the_original_bytes() {
        let parts = vec![
            (0u8..37).collect::<Vec<u8>>(),
            (37u8..40).collect(),
            (40u8..200).collect(),
            (200u8..255).collect(),
        ];
        let expected: Vec<u8> = parts.iter().flatten().copied().collect();
        let chunks = collect(chunked(stream_of(parts), 64)).await.unwrap();
        let joined: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(joined, expected, "download must be byte-identical");
    }

    #[tokio::test]
    async fn exact_multiple_of_the_chunk_size_emits_no_empty_trailer() {
        let chunks = collect(chunked(stream_of(vec![vec![1u8; 512]]), 256))
            .await
            .unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|c| c.len() == 256));
    }

    #[tokio::test]
    async fn an_empty_object_yields_no_messages() {
        let chunks = collect(chunked(stream_of(vec![]), 256)).await.unwrap();
        assert!(chunks.is_empty());

        let chunks = collect(chunked(stream_of(vec![vec![], vec![], vec![]]), 256))
            .await
            .unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn a_storage_error_surfaces_as_a_stream_error() {
        let result = collect(chunked(
            failing_stream(vec![vec![1u8; 300]], "s3 connection reset"),
            256,
        ))
        .await;
        let err = result.expect_err("the storage failure must not be swallowed");
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("s3 connection reset"), "{err}");
    }

    #[tokio::test]
    async fn chunks_already_sent_before_a_failure_are_delivered() {
        let mut stream = Box::pin(chunked(
            failing_stream(vec![vec![9u8; 600]], "truncated"),
            256,
        ));
        assert_eq!(stream.next().await.unwrap().unwrap().chunk.len(), 256);
        assert_eq!(stream.next().await.unwrap().unwrap().chunk.len(), 256);
        let err = stream.next().await.unwrap().expect_err("expected failure");
        assert!(err.message().contains("truncated"));
        assert!(stream.next().await.is_none(), "stream must end after error");
    }

    #[tokio::test]
    async fn the_production_chunk_size_is_the_legacy_wire_size() {
        assert_eq!(DOWNLOAD_CHUNK_SIZE, 1024 * 1024);
        let chunks = collect(chunked(
            stream_of(vec![vec![0u8; DOWNLOAD_CHUNK_SIZE + 7]]),
            DOWNLOAD_CHUNK_SIZE,
        ))
        .await
        .unwrap();
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            vec![DOWNLOAD_CHUNK_SIZE, 7]
        );
    }

    #[tokio::test]
    async fn the_first_message_is_not_held_back_for_a_full_chunk() {
        let parts = futures::stream::iter(vec![Ok(bytes::Bytes::from(vec![1u8; 10]))])
            .chain(futures::stream::pending());
        let mut stream = Box::pin(chunked(Box::pin(parts), 1024 * 1024));
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("the first message must not wait for more bytes")
            .unwrap()
            .unwrap();
        assert_eq!(first.chunk.len(), 10);
        assert_eq!(first.total_size, 4242);
    }

    #[tokio::test]
    async fn messages_carry_their_offset_and_only_the_first_names_the_artifact() {
        let mut stream = Box::pin(chunked_download(
            stream_of(vec![vec![5u8; 300], vec![6u8; 300]]),
            256,
            8 * 1024 * 1024,
            artifact(),
        ));
        let mut messages = Vec::new();
        while let Some(m) = stream.next().await {
            messages.push(m.unwrap());
        }
        let offsets: Vec<u64> = messages.iter().map(|m| m.offset).collect();
        let base = 8 * 1024 * 1024;
        assert_eq!(offsets, vec![base, base + 256, base + 512]);
        assert_eq!(messages[0].total_size, 4242);
        assert_eq!(messages[0].sha256, "ab".repeat(32));
        assert!(
            messages[1..]
                .iter()
                .all(|m| m.total_size == 0 && m.sha256.is_empty())
        );
    }
}
