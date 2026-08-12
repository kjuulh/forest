use std::pin::Pin;

use anyhow::Context;
use forest_grpc_interface::{registry_service_server::RegistryService, *};
use futures::{Stream, StreamExt};
use uuid::Uuid;

use crate::{
    actor::Actor,
    grpc::authorize::{self, OrgRole},
    services::component_aggregate::{ComponentServiceState, ComponentVersion, FileStream},
    state::State,
};

/// gRPC message payload size for binary downloads.
///
/// Unchanged from the pre-DATA-505 implementation on purpose — the wire shape
/// stays byte-for-byte what older clients already expect. What changed is that
/// these are now cut from a live S3 stream instead of from a fully buffered
/// copy of the artifact.
const DOWNLOAD_CHUNK_SIZE: usize = 1024 * 1024;

/// Re-cut an object-storage byte stream into fixed-size gRPC messages.
///
/// S3 hands back HTTP body chunks of whatever size it likes (often tens of
/// KiB). Emitting one gRPC message per S3 chunk would multiply per-message
/// framing overhead, so chunks are coalesced up to `chunk_size` and flushed as
/// they fill; the final message carries the remainder. An empty object yields
/// no messages, exactly as before.
///
/// Pulling from S3 only happens when the consumer polls, so tonic's HTTP/2
/// flow control backpressures all the way through to object storage and memory
/// stays bounded at roughly one chunk.
fn chunked_download(
    bytes: crate::object_store::ByteStream,
    chunk_size: usize,
) -> impl Stream<Item = std::result::Result<DownloadBinaryResponse, tonic::Status>> + Send {
    struct Cursor {
        bytes: crate::object_store::ByteStream,
        buffer: Vec<u8>,
        finished: bool,
    }

    futures::stream::unfold(
        Cursor {
            bytes,
            buffer: Vec::with_capacity(chunk_size),
            finished: false,
        },
        move |mut cursor| async move {
            if cursor.finished {
                return None;
            }
            loop {
                if cursor.buffer.len() >= chunk_size {
                    let remainder = cursor.buffer.split_off(chunk_size);
                    let chunk = std::mem::replace(&mut cursor.buffer, remainder);
                    return Some((Ok(DownloadBinaryResponse { chunk }), cursor));
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
                        return Some((Ok(DownloadBinaryResponse { chunk }), cursor));
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
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &request.organisation,
            OrgRole::Member,
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
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member)
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
        // TASKS/025: org member is sufficient — by symmetry with publish,
        // anyone who can publish can unpublish. Admin-only escalation
        // would create asymmetry that locks publishers out of cleaning
        // their own mistakes.
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &request.organisation,
            OrgRole::Member,
        )
        .await?;

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

        // Tolerate unknown uploads: skip auth on no-op so client retries are
        // safe even after server-side GC. If the upload exists, auth must pass.
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
                ));
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
                    ));
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
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member)
            .await?;

        // Stream the object out of S3 rather than buffering it (DATA-505).
        //
        // This used to `download_binary(..)` the whole artifact into a `Vec`
        // and then `.chunks(1 MiB).collect()` it into a second full copy of
        // the file as a vector of ready-made messages. Time-to-first-byte was
        // therefore the entire S3 GET, and peak RSS was ~2x the artifact per
        // concurrent download — which is what made parallel downloads on the
        // client side unattractive.
        //
        // The wire format is deliberately unchanged: still `DownloadBinary`,
        // still ~1 MiB `DownloadBinaryResponse` messages. Older clients cannot
        // tell the difference.
        let byte_stream = self
            .state
            .component_service()
            .download_binary_stream(
                &req.organisation,
                &req.name,
                &req.version,
                &req.os,
                &req.arch,
            )
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(Box::pin(chunked_download(
            byte_stream,
            DOWNLOAD_CHUNK_SIZE,
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
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member)
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
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member)
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

    // --- Global-tools (TASKS/018-global-tools.md §1a.2c) ---

    type ListOrgToolsStream =
        Pin<Box<dyn Stream<Item = std::result::Result<OrgToolEntry, tonic::Status>> + Send>>;

    async fn list_org_tools(
        &self,
        request: tonic::Request<ListOrgToolsRequest>,
    ) -> std::result::Result<tonic::Response<Self::ListOrgToolsStream>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(&self.state.db, &actor, &req.organisation, OrgRole::Member)
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
            None => vec![],                               // anonymous
            Some(Actor::ServiceAccount { .. }) => vec![], // sees all via public_only=false
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

    // --- Public (unauthenticated) registry RPCs ---
    //
    // These never read the caller's identity. They always pass
    // `see_all=false, member_orgs=[]` to the component service (search)
    // or check `visibility = 'public'` explicitly (detail / manifest).
    // The auth middleware in `auth_layer.rs` marks them as `AuthMode::None`
    // so even an attached bearer token is ignored — by construction a
    // misconfigured forage cannot escalate a service-account key into
    // cross-org read access via these endpoints.

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
            .search_components(
                &req.query,
                &req.organisation,
                page_size,
                offset,
                /* see_all = */ false,
                /* member_orgs = */ &[],
            )
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

        let is_public = is_public_project(&self.state.db, &req.organisation, &req.name).await?;
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

        let is_public = is_public_project(&self.state.db, &req.organisation, &req.name).await?;
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

/// Shared helper for the public RPCs. Returns `true` only when a row in
/// `projects` exists with `visibility = 'public'`. A missing project row
/// is treated as private (matches the default behaviour in
/// `search_components` and `get_component_detail`).
async fn is_public_project(
    db: &sqlx::PgPool,
    organisation: &str,
    name: &str,
) -> Result<bool, tonic::Status> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM projects p
            WHERE p.organisation = $1 AND p.project = $2 AND p.visibility = 'public'
        )",
    )
    .bind(organisation)
    .bind(name)
    .fetch_one(db)
    .await
    .map_err(|e| tonic::Status::internal(e.to_string()))
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
    let org: String = sqlx::query_scalar("SELECT organisation FROM components WHERE id = $1")
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

#[cfg(test)]
mod download_chunking_tests {
    use super::*;

    /// Build a `ByteStream` from pre-baked parts, standing in for whatever
    /// chunk boundaries S3 happens to hand us.
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
    async fn coalesces_small_parts_into_full_chunks() {
        // 10 parts of 100 bytes, cut at 256 → 256 + 256 + 256 + 232.
        let parts = (0..10).map(|i| vec![i as u8; 100]).collect();
        let chunks = collect(chunked_download(stream_of(parts), 256))
            .await
            .unwrap();
        let sizes: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        assert_eq!(sizes, vec![256, 256, 256, 232]);
        assert_eq!(sizes.iter().sum::<usize>(), 1000);
    }

    #[tokio::test]
    async fn splits_parts_larger_than_the_chunk_size() {
        let chunks = collect(chunked_download(stream_of(vec![vec![7u8; 1000]]), 256))
            .await
            .unwrap();
        let sizes: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        assert_eq!(sizes, vec![256, 256, 256, 232]);
    }

    #[tokio::test]
    async fn reassembles_to_exactly_the_original_bytes() {
        // Awkward part boundaries, none aligned to the chunk size.
        let parts = vec![
            (0u8..37).collect::<Vec<u8>>(),
            (37u8..40).collect(),
            (40u8..200).collect(),
            (200u8..255).collect(),
        ];
        let expected: Vec<u8> = parts.iter().flatten().copied().collect();
        let chunks = collect(chunked_download(stream_of(parts), 64))
            .await
            .unwrap();
        let joined: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(joined, expected, "download must be byte-identical");
    }

    #[tokio::test]
    async fn exact_multiple_of_the_chunk_size_emits_no_empty_trailer() {
        let chunks = collect(chunked_download(stream_of(vec![vec![1u8; 512]]), 256))
            .await
            .unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|c| c.len() == 256));
    }

    #[tokio::test]
    async fn an_empty_object_yields_no_messages() {
        // Matches the pre-DATA-505 behaviour: `[].chunks(n)` produced nothing.
        let chunks = collect(chunked_download(stream_of(vec![]), 256))
            .await
            .unwrap();
        assert!(chunks.is_empty());

        // Empty parts in the middle of a stream must not produce empty
        // messages either.
        let chunks = collect(chunked_download(
            stream_of(vec![vec![], vec![], vec![]]),
            256,
        ))
        .await
        .unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn a_storage_error_surfaces_as_a_stream_error() {
        let result = collect(chunked_download(
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
        // A mid-transfer S3 failure must not retroactively hide the bytes the
        // client already received — it has to arrive as a stream error after
        // them, so the client's sha check is what rejects the partial file.
        let mut stream = Box::pin(chunked_download(
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
        // Old clients were written against 1 MiB messages; keep it that way.
        assert_eq!(DOWNLOAD_CHUNK_SIZE, 1024 * 1024);
        let chunks = collect(chunked_download(
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
}
