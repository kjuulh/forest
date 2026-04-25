//! Fake `RegistryService` for orchestrator-style tests where the guest
//! pulls components from a live registry on cache miss. Mirrors the
//! shape `forest-server` exposes — same proto, same wire format — but
//! implements only the v2 binary RPCs the runner cares about. Anything
//! older (file-based v1 components, search/discovery) returns
//! `Unimplemented` so a stray call from a misconfigured tool fails
//! loudly instead of silently passing.
//!
//! Usage:
//!
//!     let registry = FakeRegistry::start().await?;
//!     registry.upload(/* org */, /* name */, /* version */, binary_bytes, manifest_json);
//!     // … point clients at registry.endpoint() …

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use forest_grpc_interface::{
    BeginUploadRequest, BeginUploadResponse, CommitUploadRequest, CommitUploadResponse,
    DownloadBinaryRequest, DownloadBinaryResponse, GetComponentDetailRequest,
    GetComponentDetailResponse, GetComponentFilesRequest, GetComponentFilesResponse,
    GetComponentManifestRequest, GetComponentManifestResponse, GetComponentRequest,
    GetComponentResponse, GetComponentVersionRequest, GetComponentVersionResponse,
    GetComponentsRequest, GetComponentsResponse, ListComponentVersionsRequest,
    ListComponentVersionsResponse, PublishManifestRequest, PublishManifestResponse,
    SearchComponentsRequest, SearchComponentsResponse, UploadBinaryRequest,
    UploadBinaryResponse, UploadFileRequest, UploadFileResponse,
    registry_service_server::{RegistryService, RegistryServiceServer},
};
use futures::Stream;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, Streaming};

/// One stored component + its manifest, keyed by (org, name, version).
#[derive(Debug, Clone)]
struct StoredComponent {
    binary: Vec<u8>,
    manifest_json: String,
}

#[derive(Debug, Default)]
struct State {
    /// Key: `org/name/version`
    components: HashMap<String, StoredComponent>,
}

/// Test-side handle. Cheaply cloneable for test setup.
#[derive(Clone)]
pub struct FakeRegistry {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    shutdown: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl FakeRegistry {
    /// Boot a tonic server on a free port. Returns once it's listening.
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let state = Arc::new(Mutex::new(State::default()));

        let service = RegistryServiceImpl {
            state: state.clone(),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let incoming = TcpListenerStream::new(listener);
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(RegistryServiceServer::new(service))
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Ok(Self {
            addr,
            state,
            shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
        })
    }

    /// `http://127.0.0.1:<port>` — what tonic clients want.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Pre-load a component into the registry. Tests call this before
    /// any download attempts. `manifest_json` is the JSON form the
    /// runner reads via `meta.json` — the same shape we already write
    /// into the file cache.
    pub fn upload(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
        binary: Vec<u8>,
        manifest_json: String,
    ) {
        let key = format!("{organisation}/{name}/{version}");
        self.state.lock().expect("registry state").components.insert(
            key,
            StoredComponent {
                binary,
                manifest_json,
            },
        );
    }
}

impl Drop for FakeRegistry {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.lock().expect("registry shutdown").take() {
            let _ = tx.send(());
        }
    }
}

#[derive(Clone)]
struct RegistryServiceImpl {
    state: Arc<Mutex<State>>,
}

#[tonic::async_trait]
impl RegistryService for RegistryServiceImpl {
    // ---- v2 binary RPCs (the ones the runner / forest CLI care about) ----

    type DownloadBinaryStream =
        Pin<Box<dyn Stream<Item = Result<DownloadBinaryResponse, Status>> + Send + 'static>>;

    async fn download_binary(
        &self,
        request: Request<DownloadBinaryRequest>,
    ) -> Result<Response<Self::DownloadBinaryStream>, Status> {
        let req = request.into_inner();
        let key = format!("{}/{}/{}", req.organisation, req.name, req.version);
        let bytes = {
            let state = self.state.lock().expect("registry state");
            state
                .components
                .get(&key)
                .ok_or_else(|| Status::not_found(format!("no component {key}")))?
                .binary
                .clone()
        };
        // 64 KiB chunks — large enough that even a multi-MB binary
        // streams in a handful of frames, small enough to feel like
        // a real wire transfer.
        const CHUNK: usize = 64 * 1024;
        let stream = async_stream::stream! {
            for chunk in bytes.chunks(CHUNK) {
                yield Ok(DownloadBinaryResponse { chunk: chunk.to_vec() });
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_component_manifest(
        &self,
        request: Request<GetComponentManifestRequest>,
    ) -> Result<Response<GetComponentManifestResponse>, Status> {
        let req = request.into_inner();
        let key = format!("{}/{}/{}", req.organisation, req.name, req.version);
        let state = self.state.lock().expect("registry state");
        let comp = state
            .components
            .get(&key)
            .ok_or_else(|| Status::not_found(format!("no component {key}")))?;
        Ok(Response::new(GetComponentManifestResponse {
            manifest_json: comp.manifest_json.clone(),
        }))
    }

    async fn list_component_versions(
        &self,
        _request: Request<ListComponentVersionsRequest>,
    ) -> Result<Response<ListComponentVersionsResponse>, Status> {
        // Returning an empty list is the closest "not yet wired up" we
        // can get without misleading the caller. Tests that need this
        // can extend the impl when they exercise it.
        Ok(Response::new(ListComponentVersionsResponse {
            versions: Vec::new(),
        }))
    }

    async fn upload_binary(
        &self,
        request: Request<Streaming<UploadBinaryRequest>>,
    ) -> Result<Response<UploadBinaryResponse>, Status> {
        // The fake registry is pre-populated by the test harness, so
        // upload via gRPC is unsupported. Tests that publish from the
        // VM will need to flesh this out.
        let _ = request;
        Err(Status::unimplemented(
            "upload_binary not implemented in FakeRegistry",
        ))
    }

    async fn publish_manifest(
        &self,
        _request: Request<PublishManifestRequest>,
    ) -> Result<Response<PublishManifestResponse>, Status> {
        Err(Status::unimplemented(
            "publish_manifest not implemented in FakeRegistry",
        ))
    }

    // ---- everything else: explicit Unimplemented ----

    async fn get_components(
        &self,
        _request: Request<GetComponentsRequest>,
    ) -> Result<Response<GetComponentsResponse>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    async fn get_component(
        &self,
        _request: Request<GetComponentRequest>,
    ) -> Result<Response<GetComponentResponse>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    async fn get_component_version(
        &self,
        _request: Request<GetComponentVersionRequest>,
    ) -> Result<Response<GetComponentVersionResponse>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    async fn begin_upload(
        &self,
        _request: Request<BeginUploadRequest>,
    ) -> Result<Response<BeginUploadResponse>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    async fn upload_file(
        &self,
        _request: Request<UploadFileRequest>,
    ) -> Result<Response<UploadFileResponse>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    async fn commit_upload(
        &self,
        _request: Request<CommitUploadRequest>,
    ) -> Result<Response<CommitUploadResponse>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    type GetComponentFilesStream =
        Pin<Box<dyn Stream<Item = Result<GetComponentFilesResponse, Status>> + Send + 'static>>;

    async fn get_component_files(
        &self,
        _request: Request<GetComponentFilesRequest>,
    ) -> Result<Response<Self::GetComponentFilesStream>, Status> {
        Err(Status::unimplemented("v1 file components not supported"))
    }

    async fn search_components(
        &self,
        _request: Request<SearchComponentsRequest>,
    ) -> Result<Response<SearchComponentsResponse>, Status> {
        Err(Status::unimplemented("search not implemented in FakeRegistry"))
    }

    async fn get_component_detail(
        &self,
        _request: Request<GetComponentDetailRequest>,
    ) -> Result<Response<GetComponentDetailResponse>, Status> {
        Err(Status::unimplemented("detail not implemented in FakeRegistry"))
    }
}
