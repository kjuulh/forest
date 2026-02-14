use std::{collections::HashMap, path::Path, sync::OnceLock};

use anyhow::Context;
use forest_grpc_interface::{
    artifact_service_client::ArtifactServiceClient,
    destination_service_client::DestinationServiceClient, get_component_files_response::Msg,
    get_projects_request::Query,
    organisation_service_client::OrganisationServiceClient,
    registry_service_client::RegistryServiceClient, release_service_client::ReleaseServiceClient,
    users_service_client::UsersServiceClient, *,
};
use forest_models::{Destination, DestinationType, OrganisationName, ProjectName};
use futures::{SinkExt, Stream, TryStreamExt};
use tokio::{
    sync::{OnceCell, mpsc::Sender},
    task::JoinHandle,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    Response,
    transport::{Channel, ClientTlsConfig},
};
use tower::ServiceBuilder;
use uuid::Uuid;

use crate::{
    grpc::interceptor::{AuthMiddleware, AuthMiddlewareLayer, AuthMiddlewareLayerState},
    models::{
        self, artifacts::ArtifactID, context::ArtifactContext,
        release_annotation::ReleaseAnnotation, source::Source,
    },
    state::State,
};

mod interceptor;

/// Convert a `tonic::Status` into a clean `anyhow::Error` containing only the
/// human-readable message (e.g. "member already exists in this organisation")
/// rather than the raw debug format with status codes and metadata.
fn grpc_err(status: tonic::Status) -> anyhow::Error {
    anyhow::anyhow!("{}", status.message())
}

#[derive(Clone)]
pub struct GrpcClient {
    host: String,
    auth_middleware_layer: AuthMiddlewareLayer,

    channel: OnceCell<Channel>,
    registry_client: OnceCell<RegistryServiceClient<AuthMiddleware<Channel>>>,
    artifact_client: OnceCell<ArtifactServiceClient<AuthMiddleware<Channel>>>,
    release_client: OnceCell<ReleaseServiceClient<AuthMiddleware<Channel>>>,
    destination_client: OnceCell<DestinationServiceClient<AuthMiddleware<Channel>>>,
    organisation_client: OnceCell<OrganisationServiceClient<AuthMiddleware<Channel>>>,
    users_client: OnceCell<UsersServiceClient<Channel>>,
    auth_users_client: OnceCell<UsersServiceClient<AuthMiddleware<Channel>>>,
}

impl GrpcClient {
    pub async fn get_component(
        &self,
        name: &str,
        organisation: &str,
    ) -> anyhow::Result<Option<Component>> {
        let mut client = self.registry_client().await?;

        let resp = client
            .get_component(GetComponentRequest {
                name: name.into(),
                organisation: organisation.into(),
            })
            .await
            .map_err(grpc_err)?;

        let resp = resp.into_inner();

        Ok(resp.component)
    }

    pub async fn get_component_version(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
    ) -> anyhow::Result<Option<Component>> {
        let mut client = self.registry_client().await?;

        let resp = client
            .get_component_version(GetComponentVersionRequest {
                name: name.into(),
                organisation: organisation.into(),
                version: version.into(),
            })
            .await
            .map_err(grpc_err)?;

        let resp = resp.into_inner();

        Ok(resp.component)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn begin_upload(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
    ) -> anyhow::Result<UploadContext> {
        let mut client = self.registry_client().await?;

        tracing::debug!("beginning upload");

        let res = client
            .begin_upload(BeginUploadRequest {
                name: name.into(),
                organisation: organisation.into(),
                version: version.into(),
            })
            .await
            .map_err(grpc_err)?;

        Ok(UploadContext {
            context_id: res.into_inner().upload_context.parse()?,
        })
    }

    #[tracing::instrument(skip(self, file_path, file_content), level = "trace")]
    pub async fn upload_file(
        &self,
        context: &UploadContext,
        file_path: &Path,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        let mut client = self.registry_client().await?;

        tracing::debug!("uploading file");

        client
            .upload_file(UploadFileRequest {
                upload_context: context.into(),
                file_path: file_path.to_string_lossy().to_string(),
                file_content: file_content.into(),
            })
            .await
            .map_err(grpc_err)?;

        Ok(())
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn commit_upload(&self, context: &UploadContext) -> anyhow::Result<()> {
        let mut client = self.registry_client().await?;

        tracing::debug!("commit upload");

        client
            .commit_upload(CommitUploadRequest {
                upload_context: context.into(),
            })
            .await
            .map_err(grpc_err)?;

        Ok(())
    }

    pub async fn begin_artifact_upload(&self) -> anyhow::Result<UploadFileHandle> {
        let mut client = self.artifact_client().await?;

        let resp = client
            .begin_upload_artifact(BeginUploadArtifactRequest {})
            .await
            .map_err(grpc_err)?;

        let resp = resp.into_inner();

        let (tx, rx) = tokio::sync::mpsc::channel::<UploadArtifactRequest>(10);

        let handle = tokio::spawn(async move {
            let stream_req = ReceiverStream::new(rx);
            client
                .upload_artifact(stream_req)
                .await
                .map_err(grpc_err)
                .context("upload artifact")
                .inspect_err(|e| tracing::error!("failed to upload file: {:?}", e))
        });

        Ok(UploadFileHandle {
            tx,
            handle,
            staging_id: resp.upload_id,
        })
    }

    #[tracing::instrument(skip(self, handle, file_content), level = "trace")]
    pub async fn upload_artifact_file(
        &self,
        handle: &UploadFileHandle,
        file_name: &str,
        file_content: &str,
        env: &str,
        destination: &str,
    ) -> anyhow::Result<()> {
        tracing::info!("uploading file: {}", handle.staging_id);

        handle
            .tx
            .send(UploadArtifactRequest {
                upload_id: handle.staging_id.clone(),
                env: env.into(),
                destination: destination.into(),
                file_name: file_name.into(),
                file_content: file_content.into(),
            })
            .await?;

        Ok(())
    }

    pub async fn commit_artifact_upload(
        &self,
        handle: UploadFileHandle,
    ) -> anyhow::Result<ArtifactID> {
        let staging_id = handle.staging_id;
        let upload_file_handle = handle.handle;

        drop(handle.tx);

        // Make sure we've received a final response from the server
        // FIXME: this may block forever if there are multiple producers
        upload_file_handle.await??;

        tracing::info!("commiting upload: {}", staging_id);

        let mut client = self.artifact_client().await?;
        let res = client
            .commit_artifact(CommitArtifactRequest {
                upload_id: staging_id,
            })
            .await
            .map_err(grpc_err)
            .context("grpc commit artifact")?;

        let msg = res.into_inner();

        Ok(msg.artifact_id.try_into()?)
    }

    async fn channel(&self) -> anyhow::Result<Channel> {
        let channel = self
            .channel
            .get_or_try_init(move || async move {
                let channel = Channel::from_shared(self.host.clone())?
                    .tls_config(ClientTlsConfig::new().with_enabled_roots())?
                    .connect()
                    .await?;

                Ok::<_, anyhow::Error>(channel)
            })
            .await?;

        Ok(channel.clone())
    }

    fn auth_channel(&self, channel: Channel) -> AuthMiddleware<Channel> {
        ServiceBuilder::new()
            .layer(self.auth_middleware_layer.clone())
            .service(channel)
    }

    async fn release_client(
        &self,
    ) -> anyhow::Result<ReleaseServiceClient<AuthMiddleware<Channel>>> {
        let client = self
            .release_client
            .get_or_try_init(move || async move {
                let channel = self.auth_channel(self.channel().await?);
                Ok::<_, anyhow::Error>(ReleaseServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    async fn registry_client(
        &self,
    ) -> anyhow::Result<RegistryServiceClient<AuthMiddleware<Channel>>> {
        let client = self
            .registry_client
            .get_or_try_init(move || async move {
                let channel = self.auth_channel(self.channel().await?);
                Ok::<_, anyhow::Error>(RegistryServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    async fn artifact_client(
        &self,
    ) -> anyhow::Result<ArtifactServiceClient<AuthMiddleware<Channel>>> {
        let client = self
            .artifact_client
            .get_or_try_init(move || async move {
                let channel = self.auth_channel(self.channel().await?);
                Ok::<_, anyhow::Error>(ArtifactServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    async fn destination_client(
        &self,
    ) -> anyhow::Result<DestinationServiceClient<AuthMiddleware<Channel>>> {
        let client = self
            .destination_client
            .get_or_try_init(move || async move {
                let channel = self.auth_channel(self.channel().await?);
                Ok::<_, anyhow::Error>(DestinationServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    async fn organisation_client(
        &self,
    ) -> anyhow::Result<OrganisationServiceClient<AuthMiddleware<Channel>>> {
        let client = self
            .organisation_client
            .get_or_try_init(move || async move {
                let channel = self.auth_channel(self.channel().await?);
                Ok::<_, anyhow::Error>(OrganisationServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    // ── Organisations ───────────────────────────────────────────────

    pub async fn create_organisation(
        &self,
        name: &str,
    ) -> anyhow::Result<CreateOrganisationResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .create_organisation(CreateOrganisationRequest {
                name: name.into(),
            })
            .await
            .map_err(grpc_err)
            .context("create organisation")?;
        Ok(resp.into_inner())
    }

    pub async fn get_organisation(
        &self,
        identifier: get_organisation_request::Identifier,
    ) -> anyhow::Result<Option<Organisation>> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .get_organisation(GetOrganisationRequest {
                identifier: Some(identifier),
            })
            .await
            .map_err(grpc_err)
            .context("get organisation")?;
        Ok(resp.into_inner().organisation)
    }

    pub async fn search_organisations(
        &self,
        query: &str,
        page_size: i32,
        page_token: &str,
    ) -> anyhow::Result<SearchOrganisationsResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .search_organisations(SearchOrganisationsRequest {
                query: query.into(),
                page_size,
                page_token: page_token.into(),
            })
            .await
            .map_err(grpc_err)
            .context("search organisations")?;
        Ok(resp.into_inner())
    }

    pub async fn list_my_organisations(
        &self,
        role: &str,
    ) -> anyhow::Result<ListMyOrganisationsResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .list_my_organisations(ListMyOrganisationsRequest {
                role: role.into(),
            })
            .await
            .map_err(grpc_err)
            .context("list my organisations")?;
        Ok(resp.into_inner())
    }

    // -- Organisation Members -----------------------------------------------------

    pub async fn add_organisation_member(
        &self,
        organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> anyhow::Result<AddMemberResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .add_member(AddMemberRequest {
                organisation_id: organisation_id.into(),
                user_id: user_id.into(),
                role: role.into(),
            })
            .await
            .map_err(grpc_err)
            .context("add organisation member")?;
        Ok(resp.into_inner())
    }

    pub async fn remove_organisation_member(
        &self,
        organisation_id: &str,
        user_id: &str,
    ) -> anyhow::Result<RemoveMemberResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .remove_member(RemoveMemberRequest {
                organisation_id: organisation_id.into(),
                user_id: user_id.into(),
            })
            .await
            .map_err(grpc_err)
            .context("remove organisation member")?;
        Ok(resp.into_inner())
    }

    pub async fn update_organisation_member_role(
        &self,
        organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> anyhow::Result<UpdateMemberRoleResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .update_member_role(UpdateMemberRoleRequest {
                organisation_id: organisation_id.into(),
                user_id: user_id.into(),
                role: role.into(),
            })
            .await
            .map_err(grpc_err)
            .context("update organisation member role")?;
        Ok(resp.into_inner())
    }

    pub async fn list_organisation_members(
        &self,
        organisation_id: &str,
        page_size: i32,
        page_token: &str,
    ) -> anyhow::Result<ListMembersResponse> {
        let mut client = self.organisation_client().await?;
        let resp = client
            .list_members(ListMembersRequest {
                organisation_id: organisation_id.into(),
                page_size,
                page_token: page_token.into(),
            })
            .await
            .map_err(grpc_err)
            .context("list organisation members")?;
        Ok(resp.into_inner())
    }

    /// Unauthenticated users client (login, register, refresh_token).
    async fn users_client(&self) -> anyhow::Result<UsersServiceClient<Channel>> {
        let client = self
            .users_client
            .get_or_try_init(move || async move {
                let channel = self.channel().await?;
                Ok::<_, anyhow::Error>(UsersServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    /// Authenticated users client (get_user, update, delete, PATs, etc.).
    async fn auth_users_client(
        &self,
    ) -> anyhow::Result<UsersServiceClient<AuthMiddleware<Channel>>> {
        let client = self
            .auth_users_client
            .get_or_try_init(move || async move {
                let channel = self.auth_channel(self.channel().await?);
                Ok::<_, anyhow::Error>(UsersServiceClient::new(channel))
            })
            .await?;

        Ok(client.clone())
    }

    // ── Users / Auth ─────────────────────────────────────────────────

    pub async fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> anyhow::Result<RegisterResponse> {
        let mut client = self.users_client().await?;
        let resp = client
            .register(RegisterRequest {
                username: username.into(),
                email: email.into(),
                password: password.into(),
            })
            .await
            .map_err(grpc_err)
            .context("register")?;
        Ok(resp.into_inner())
    }

    pub async fn login(
        &self,
        identifier: login_request::Identifier,
        password: &str,
    ) -> anyhow::Result<LoginResponse> {
        let mut client = self.users_client().await?;
        let resp = client
            .login(LoginRequest {
                identifier: Some(identifier),
                password: password.into(),
            })
            .await
            .map_err(grpc_err)
            .context("login")?;
        Ok(resp.into_inner())
    }

    pub async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> anyhow::Result<RefreshTokenResponse> {
        let mut client = self.users_client().await?;
        let resp = client
            .refresh_token(RefreshTokenRequest {
                refresh_token: refresh_token.into(),
            })
            .await
            .map_err(grpc_err)
            .context("refresh token")?;
        Ok(resp.into_inner())
    }

    pub async fn logout(&self, refresh_token: &str) -> anyhow::Result<()> {
        let mut client = self.auth_users_client().await?;
        client
            .logout(LogoutRequest {
                refresh_token: refresh_token.into(),
            })
            .await
            .map_err(grpc_err)
            .context("logout")?;
        Ok(())
    }

    pub async fn get_user(
        &self,
        identifier: get_user_request::Identifier,
    ) -> anyhow::Result<Option<User>> {
        let mut client = self.auth_users_client().await?;
        let resp = client
            .get_user(GetUserRequest {
                identifier: Some(identifier),
            })
            .await
            .map_err(grpc_err)
            .context("get user")?;
        Ok(resp.into_inner().user)
    }

    pub async fn update_user(
        &self,
        user_id: &str,
        username: Option<String>,
    ) -> anyhow::Result<Option<User>> {
        let mut client = self.auth_users_client().await?;
        let resp = client
            .update_user(UpdateUserRequest {
                user_id: user_id.into(),
                username,
            })
            .await
            .map_err(grpc_err)
            .context("update user")?;
        Ok(resp.into_inner().user)
    }

    pub async fn delete_user(&self, user_id: &str) -> anyhow::Result<()> {
        let mut client = self.auth_users_client().await?;
        client
            .delete_user(DeleteUserRequest {
                user_id: user_id.into(),
            })
            .await
            .map_err(grpc_err)
            .context("delete user")?;
        Ok(())
    }

    pub async fn list_users(
        &self,
        page_size: i32,
        page_token: &str,
        search: Option<String>,
    ) -> anyhow::Result<ListUsersResponse> {
        let mut client = self.auth_users_client().await?;
        let resp = client
            .list_users(ListUsersRequest {
                page_size,
                page_token: page_token.into(),
                search,
            })
            .await
            .map_err(grpc_err)
            .context("list users")?;
        Ok(resp.into_inner())
    }

    pub async fn change_password(
        &self,
        user_id: &str,
        current_password: &str,
        new_password: &str,
    ) -> anyhow::Result<()> {
        let mut client = self.auth_users_client().await?;
        client
            .change_password(ChangePasswordRequest {
                user_id: user_id.into(),
                current_password: current_password.into(),
                new_password: new_password.into(),
            })
            .await
            .map_err(grpc_err)
            .context("change password")?;
        Ok(())
    }

    pub async fn create_personal_access_token(
        &self,
        user_id: &str,
        name: &str,
        scopes: Vec<String>,
        expires_in_seconds: i64,
    ) -> anyhow::Result<CreatePersonalAccessTokenResponse> {
        let mut client = self.auth_users_client().await?;
        let resp = client
            .create_personal_access_token(CreatePersonalAccessTokenRequest {
                user_id: user_id.into(),
                name: name.into(),
                scopes,
                expires_in_seconds,
            })
            .await
            .map_err(grpc_err)
            .context("create personal access token")?;
        Ok(resp.into_inner())
    }

    pub async fn list_personal_access_tokens(
        &self,
        user_id: &str,
    ) -> anyhow::Result<Vec<PersonalAccessToken>> {
        let mut client = self.auth_users_client().await?;
        let resp = client
            .list_personal_access_tokens(ListPersonalAccessTokensRequest {
                user_id: user_id.into(),
            })
            .await
            .map_err(grpc_err)
            .context("list personal access tokens")?;
        Ok(resp.into_inner().tokens)
    }

    pub async fn delete_personal_access_token(&self, token_id: &str) -> anyhow::Result<()> {
        let mut client = self.auth_users_client().await?;
        client
            .delete_personal_access_token(DeletePersonalAccessTokenRequest {
                token_id: token_id.into(),
            })
            .await
            .map_err(grpc_err)
            .context("delete personal access token")?;
        Ok(())
    }

    pub async fn list_files(
        &self,
        component_id: &str,
        f: impl Fn(ComponentFile),
    ) -> anyhow::Result<()> {
        let mut client = self.registry_client().await?;
        let resp = client
            .get_component_files(GetComponentFilesRequest {
                component_id: component_id.into(),
            })
            .await
            .map_err(grpc_err)?;

        let mut stream = resp.into_inner();
        while let Some(msg) = stream.message().await.map_err(grpc_err)? {
            let Some(msg) = msg.msg else { return Ok(()) };

            match msg {
                Msg::Done(_) => {
                    tracing::info!("done receiving items");
                    break;
                }
                Msg::ComponentFile(component_file) => {
                    f(component_file);
                }
            }
        }

        Ok(())
    }

    pub async fn get_component_files(
        &self,
        component_id: &str,
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<ComponentFile>>> {
        let mut client = self.registry_client().await?;
        let resp = client
            .get_component_files(GetComponentFilesRequest {
                component_id: component_id.into(),
            })
            .await
            .map_err(grpc_err)?;

        let (mut tx, rx) = futures::channel::mpsc::channel(10);

        tokio::spawn(async move {
            let mut stream = resp.into_inner();
            loop {
                let Ok(message) = stream.message().await else {
                    tx.send(Err(anyhow::anyhow!("failed to read next item")))
                        .await
                        .expect("failed to send end result");
                    tx.close_channel();

                    return;
                };
                if let Some(msg) = message {
                    let Some(msg) = msg.msg else {
                        return;
                    };

                    match msg {
                        Msg::Done(_) => {
                            tracing::info!("done receiving items");
                            tx.close_channel();

                            break;
                        }
                        Msg::ComponentFile(component_file) => {
                            tx.send(Ok(component_file))
                                .await
                                .expect("to be able to send item");
                        }
                    }
                } else {
                    break;
                }
            }

            tx.close_channel();
        });

        Ok(rx.into_stream())
    }

    pub async fn annotate_artifact(
        &self,
        artifact_id: &ArtifactID,
        metadata: &HashMap<String, String>,
        source: &Source,
        context: &ArtifactContext,
        project: &models::project::Project,
        reference: &models::reference::Reference,
    ) -> anyhow::Result<String> {
        let mut client = self.release_client().await?;

        let resp = client
            .annotate_release(AnnotateReleaseRequest {
                artifact_id: artifact_id.to_string(),
                metadata: metadata.clone(),
                source: Some(source.clone().into()),
                context: Some(context.clone().into()),
                project: Some(project.clone().into()),
                r#ref: Some(reference.clone().into()),
            })
            .await
            .map_err(grpc_err)
            .context("annotate artifact")?;

        let resp = resp.into_inner();

        Ok(resp.artifact.context("no artifact found")?.slug)
    }

    pub async fn get_release_annotation_by_slug(
        &self,
        slug: &str,
    ) -> anyhow::Result<ReleaseAnnotation> {
        let mut client = self.release_client().await?;

        let resp = client
            .get_artifact_by_slug(GetArtifactBySlugRequest { slug: slug.into() })
            .await
            .map_err(grpc_err)
            .context("get release annotation by slug")?;

        let res = resp.into_inner();

        res.artifact
            .ok_or(anyhow::anyhow!("artifact could not be found"))?
            .try_into()
            .context("release annotation")
    }

    pub async fn get_release_annotations_by_project(
        &self,
        organisation: &str,
        project: &str,
    ) -> anyhow::Result<Vec<ReleaseAnnotation>> {
        let mut client = self.release_client().await?;

        let resp = client
            .get_artifacts_by_project(GetArtifactsByProjectRequest {
                project: Some(Project {
                    organisation: organisation.into(),
                    project: project.into(),
                }),
            })
            .await
            .map_err(grpc_err)
            .context("get releases by project")?;

        let res = resp.into_inner();

        res.artifact
            .into_iter()
            .map(|a| a.try_into())
            .collect::<anyhow::Result<Vec<_>>>()
    }

    pub async fn release(
        &self,
        artifact_id: Uuid,
        destination: &[String],
        environments: &[String],
    ) -> anyhow::Result<ReleaseResult> {
        let mut client = self.release_client().await?;

        let response = client
            .release(ReleaseRequest {
                artifact_id: artifact_id.to_string(),
                destinations: destination.into(),
                environments: environments.into(),
            })
            .await
            .map_err(grpc_err)
            .context("release (grpc)")?;

        let resp = response.into_inner();

        // All intents share the same release_intent_id
        let release_intent_id = resp
            .intents
            .first()
            .map(|i| i.release_intent_id.clone())
            .context("no intents returned")?;

        Ok(ReleaseResult {
            release_intent_id: release_intent_id.parse().context("release_intent_id")?,
            releases: resp
                .intents
                .into_iter()
                .map(|i| ReleaseIntentInfo {
                    destination: i.destination,
                    environment: i.environment,
                })
                .collect(),
        })
    }

    pub async fn wait_release(&self, release_intent_id: Uuid) -> anyhow::Result<WaitReleaseResult> {
        use futures::StreamExt;

        let mut client = self.release_client().await?;

        let response = client
            .wait_release(forest_grpc_interface::WaitReleaseRequest {
                release_intent_id: release_intent_id.to_string(),
            })
            .await
            .map_err(grpc_err)
            .context("wait_release (grpc)")?;

        let mut stream = response.into_inner();
        // Track status per destination
        let mut final_statuses: HashMap<String, forest_models::ReleaseStatus> = HashMap::new();

        while let Some(event) = stream.next().await {
            let event = event.map_err(grpc_err).context("stream error")?;

            match event.event {
                Some(forest_grpc_interface::wait_release_event::Event::StatusUpdate(status)) => {
                    let release_status: forest_models::ReleaseStatus = status
                        .status
                        .parse()
                        .map_err(|e| anyhow::anyhow!("{}", e))?;

                    tracing::debug!(
                        destination =% status.destination,
                        status =% release_status,
                        "received status update"
                    );

                    final_statuses.insert(status.destination, release_status);
                }
                Some(forest_grpc_interface::wait_release_event::Event::LogLine(log)) => {
                    // Print log lines to appropriate output stream
                    match forest_grpc_interface::LogChannel::try_from(log.channel) {
                        Ok(forest_grpc_interface::LogChannel::Stderr) => {
                            eprintln!("{}: {}", log.destination, log.line);
                        }
                        _ => {
                            println!("{}: {}", log.destination, log.line);
                        }
                    }
                }
                None => {}
            }
        }

        // Return aggregated results
        let destinations: Vec<WaitReleaseDestinationResult> = final_statuses
            .into_iter()
            .map(|(dest, status)| WaitReleaseDestinationResult {
                destination: dest,
                status,
            })
            .collect();

        if destinations.is_empty() {
            anyhow::bail!("stream ended without any status updates");
        }

        Ok(WaitReleaseResult { destinations })
    }

    pub async fn get_organisations(&self) -> anyhow::Result<Vec<OrganisationName>> {
        let mut client = self.release_client().await?;

        let response = client
            .get_organisations(GetOrganisationsRequest {})
            .await
            .map_err(grpc_err)
            .context("get organisations (grpc)")?;
        let resp = response.into_inner();

        Ok(resp.organisations.into_iter().map(|r| r.into()).collect())
    }

    pub async fn get_projects(&self, query: GetProjectsQuery) -> anyhow::Result<Vec<ProjectName>> {
        let query = match query {
            GetProjectsQuery::Organisation(org) => Query::Organisation(org.into()),
        };
        let mut client = self.release_client().await?;

        let response = client
            .get_projects(GetProjectsRequest {
                query: Some(query),
            })
            .await
            .map_err(grpc_err)
            .context("get projects (grpc)")?;
        let resp = response.into_inner();

        Ok(resp.projects.into_iter().map(|r| r.into()).collect())
    }

    pub async fn get_destinations(&self, organisation: &str) -> anyhow::Result<Vec<Destination>> {
        let mut client = self.destination_client().await?;

        let response = client
            .get_destinations(GetDestinationsRequest {
                organisation: organisation.to_string(),
            })
            .await
            .map_err(grpc_err)
            .context("get destinations (grpc)")?;
        let resp = response.into_inner();

        Ok(resp
            .destinations
            .into_iter()
            .map(|r| {
                Destination::new(
                    &r.organisation,
                    &r.name,
                    &r.environment,
                    r.metadata,
                    r.r#type.expect("to always be available").into(),
                )
            })
            .collect())
    }

    pub async fn create_destination(
        &self,
        organisation: &str,
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        destination_type: DestinationType,
    ) -> anyhow::Result<()> {
        self.destination_client()
            .await?
            .create_destination(CreateDestinationRequest {
                organisation: organisation.to_string(),
                name: name.to_string(),
                environment: environment.to_string(),
                metadata,
                r#type: Some(destination_type.into()),
            })
            .await
            .map_err(grpc_err)
            .context("create destination (grpc)")?;

        Ok(())
    }

    pub async fn update_destination(
        &self,
        name: &str,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.destination_client()
            .await?
            .update_destination(UpdateDestinationRequest {
                name: name.to_string(),
                metadata,
            })
            .await
            .map_err(grpc_err)
            .context("update destination (grpc)")?;

        Ok(())
    }
}

pub enum GetProjectsQuery {
    Organisation(OrganisationName),
}

pub struct ReleaseResult {
    pub release_intent_id: Uuid,
    pub releases: Vec<ReleaseIntentInfo>,
}

pub struct ReleaseIntentInfo {
    pub destination: String,
    pub environment: String,
}

pub struct WaitReleaseResult {
    pub destinations: Vec<WaitReleaseDestinationResult>,
}

impl WaitReleaseResult {
    /// Returns true if all destinations succeeded
    pub fn all_succeeded(&self) -> bool {
        self.destinations.iter().all(|d| d.status.is_success())
    }

    /// Returns true if any destination failed
    pub fn any_failed(&self) -> bool {
        self.destinations.iter().any(|d| d.status.is_failure())
    }
}

pub struct WaitReleaseDestinationResult {
    pub destination: String,
    pub status: forest_models::ReleaseStatus,
}

#[derive(Clone, Debug)]
pub struct UploadContext {
    context_id: uuid::Uuid,
}

impl From<UploadContext> for String {
    fn from(value: UploadContext) -> Self {
        value.context_id.to_string()
    }
}

impl From<&UploadContext> for String {
    fn from(value: &UploadContext) -> Self {
        value.context_id.to_string()
    }
}

pub trait GrpcClientState {
    fn grpc_client(&self) -> GrpcClient;
}

impl GrpcClientState for State {
    fn grpc_client(&self) -> GrpcClient {
        static GRPC: OnceLock<GrpcClient> = OnceLock::new();

        GRPC.get_or_init(move || {
            tracing::trace!("creating grpc client");

            GrpcClient {
                host: self.config.forest_server.clone(),
                auth_middleware_layer: self.auth_interceptor(),

                channel: OnceCell::const_new(),
                registry_client: OnceCell::const_new(),
                artifact_client: OnceCell::const_new(),
                release_client: OnceCell::const_new(),
                destination_client: OnceCell::const_new(),
                organisation_client: OnceCell::const_new(),
                users_client: OnceCell::const_new(),
                auth_users_client: OnceCell::const_new(),
            }
        })
        .clone()
    }
}

pub struct UploadFileHandle {
    tx: Sender<UploadArtifactRequest>,
    handle: JoinHandle<Result<Response<UploadArtifactResponse>, anyhow::Error>>,
    staging_id: String,
}

impl From<crate::models::context::ArtifactContext> for forest_grpc_interface::ArtifactContext {
    fn from(value: crate::models::context::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
        }
    }
}

impl From<crate::models::source::Source> for forest_grpc_interface::Source {
    fn from(value: crate::models::source::Source) -> Self {
        Self {
            user: value.username,
            email: value.email,
        }
    }
}

impl TryFrom<forest_grpc_interface::Artifact> for models::release_annotation::ReleaseAnnotation {
    type Error = anyhow::Error;

    fn try_from(value: forest_grpc_interface::Artifact) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.parse().context("id")?,
            artifact_id: value.artifact_id.parse().context("artifact id")?,
            slug: value.slug,
            metadata: value.metadata,
            source: value.source.context("source not found")?.into(),
            context: value.context.context("context not found")?.into(),
            destinations: value.destinations.into_iter().map(|d| d.into()).collect(),
            created_at: chrono::DateTime::parse_from_rfc3339(&value.created_at)
                .context("created_at")?
                .with_timezone(&chrono::Utc),
        })
    }
}

impl From<forest_grpc_interface::ArtifactDestination>
    for models::release_annotation::ReleaseDestination
{
    fn from(value: forest_grpc_interface::ArtifactDestination) -> Self {
        Self {
            name: value.name,
            environment: value.environment,
            type_organisation: value.type_organisation,
            type_name: value.type_name,
            type_version: value.type_version,
        }
    }
}

impl From<forest_grpc_interface::Source> for models::source::Source {
    fn from(value: forest_grpc_interface::Source) -> Self {
        Self {
            username: value.user,
            email: value.email,
        }
    }
}

impl From<forest_grpc_interface::ArtifactContext> for models::context::ArtifactContext {
    fn from(value: forest_grpc_interface::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
        }
    }
}

impl From<crate::models::project::Project> for forest_grpc_interface::Project {
    fn from(value: crate::models::project::Project) -> Self {
        Self {
            organisation: value.organisation,
            project: value.project,
        }
    }
}
impl From<forest_grpc_interface::Project> for crate::models::project::Project {
    fn from(value: forest_grpc_interface::Project) -> Self {
        Self {
            organisation: value.organisation,
            project: value.project,
        }
    }
}


impl From<forest_grpc_interface::Ref> for crate::models::reference::Reference {
    fn from(value: forest_grpc_interface::Ref) -> Self {
        Self {
            commit_sha: value.commit_sha,
            commit_branch: value.branch,
        }
    }
}
impl From<crate::models::reference::Reference> for forest_grpc_interface::Ref {
    fn from(value: crate::models::reference::Reference) -> Self {
        Self {
            commit_sha: value.commit_sha,
            branch: value.commit_branch,
        }
    }
}
