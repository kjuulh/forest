use forage_core::auth::{
    AddEmailResult, AuthError, AuthTokens, CreatedToken, ForestAuth, LoginResult, MfaSetup,
    PersonalAccessToken, RegisterResult, User, UserEmail, UserProfile,
};
use forage_core::platform::{
    ApprovalDecisionEntry, ApprovalState, Artifact, ArtifactContext, ArtifactDestination,
    ArtifactRef, ArtifactSource, CreatePolicyInput, CreateReleasePipelineInput, CreateTriggerInput,
    Destination, DestinationType, DestinationTypeInfo, Environment, ForestPlatform,
    MetadataFieldDef, NotificationPreference, Organisation, OrgMember, PipelineStage,
    PipelineStageConfig, PlanOutput, PlatformError, Policy, PolicyConfig, PolicyEvaluation,
    ReleasePipeline, Trigger, UpdatePolicyInput, UpdateReleasePipelineInput, UpdateTriggerInput,
};
use forage_core::registry::{
    ComponentDetail, ComponentSearchResult, ComponentSummary, ComponentVersionInfo, ForestRegistry,
    ToolSummary,
};
use forage_grpc::policy_service_client::PolicyServiceClient;
use forage_grpc::registry_service_client::RegistryServiceClient;
use forage_grpc::release_pipeline_service_client::ReleasePipelineServiceClient;
use forage_grpc::trigger_service_client::TriggerServiceClient;
use forage_grpc::destination_service_client::DestinationServiceClient;
use forage_grpc::environment_service_client::EnvironmentServiceClient;
use forage_grpc::organisation_service_client::OrganisationServiceClient;
use forage_grpc::release_service_client::ReleaseServiceClient;
use forage_grpc::users_service_client::UsersServiceClient;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::Request;

fn bearer_request<T>(access_token: &str, msg: T) -> Result<Request<T>, String> {
    let mut req = Request::new(msg);
    let bearer: MetadataValue<_> = format!("Bearer {access_token}")
        .parse()
        .map_err(|_| "invalid token format".to_string())?;
    req.metadata_mut().insert("authorization", bearer);
    Ok(req)
}

/// Real gRPC client to forest-server's UsersService.
#[derive(Clone)]
pub struct GrpcForestClient {
    channel: Channel,
    service_account_key: Option<String>,
}

impl GrpcForestClient {
    /// Create a client that connects lazily (for when server may not be available at startup).
    pub fn connect_lazy(endpoint: &str) -> anyhow::Result<Self> {
        let channel = Channel::from_shared(endpoint.to_string())?.connect_lazy();
        Ok(Self {
            channel,
            service_account_key: None,
        })
    }

    pub fn with_service_account_key(mut self, key: String) -> Self {
        self.service_account_key = Some(key);
        self
    }

    pub fn service_account_key(&self) -> Option<&str> {
        self.service_account_key.as_deref()
    }

    fn client(&self) -> UsersServiceClient<Channel> {
        UsersServiceClient::new(self.channel.clone())
    }

    fn org_client(&self) -> OrganisationServiceClient<Channel> {
        OrganisationServiceClient::new(self.channel.clone())
    }

    pub(crate) fn artifact_client(
        &self,
    ) -> forage_grpc::artifact_service_client::ArtifactServiceClient<Channel> {
        forage_grpc::artifact_service_client::ArtifactServiceClient::new(self.channel.clone())
    }

    pub(crate) fn release_client(&self) -> ReleaseServiceClient<Channel> {
        ReleaseServiceClient::new(self.channel.clone())
    }

    fn env_client(&self) -> EnvironmentServiceClient<Channel> {
        EnvironmentServiceClient::new(self.channel.clone())
    }

    fn dest_client(&self) -> DestinationServiceClient<Channel> {
        DestinationServiceClient::new(self.channel.clone())
    }

    fn trigger_client(&self) -> TriggerServiceClient<Channel> {
        TriggerServiceClient::new(self.channel.clone())
    }

    fn policy_client(&self) -> PolicyServiceClient<Channel> {
        PolicyServiceClient::new(self.channel.clone())
    }

    fn pipeline_client(&self) -> ReleasePipelineServiceClient<Channel> {
        ReleasePipelineServiceClient::new(self.channel.clone())
    }

    pub fn event_client(
        &self,
    ) -> forage_grpc::event_service_client::EventServiceClient<Channel> {
        forage_grpc::event_service_client::EventServiceClient::new(self.channel.clone())
    }

    fn registry_client(&self) -> RegistryServiceClient<Channel> {
        RegistryServiceClient::new(self.channel.clone())
    }

    pub(crate) fn notification_client(
        &self,
    ) -> forage_grpc::notification_service_client::NotificationServiceClient<Channel> {
        forage_grpc::notification_service_client::NotificationServiceClient::new(
            self.channel.clone(),
        )
    }

    fn authed_request<T>(access_token: &str, msg: T) -> Result<Request<T>, AuthError> {
        bearer_request(access_token, msg).map_err(AuthError::Other)
    }

    /// Fetch release intent states using a service token (for background workers).
    pub async fn get_release_intent_states_with_token(
        &self,
        service_token: &str,
        organisation: &str,
        project: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<forage_core::platform::ReleaseIntentState>, String> {
        let req = bearer_request(
            service_token,
            forage_grpc::GetReleaseIntentStatesRequest {
                organisation: organisation.into(),
                project: project.map(|p| p.into()),
                include_completed,
            },
        )
        .map_err(|e| format!("invalid token: {e}"))?;

        let resp = self
            .release_client()
            .get_release_intent_states(req)
            .await
            .map_err(|e| format!("gRPC: {e}"))?;

        Ok(resp
            .into_inner()
            .release_intents
            .into_iter()
            .map(|ri| forage_core::platform::ReleaseIntentState {
                release_intent_id: ri.release_intent_id,
                artifact_id: ri.artifact_id,
                project: ri.project,
                created_at: ri.created_at,
                stages: ri.stages.into_iter().map(convert_pipeline_stage_state).collect(),
                steps: ri.steps.into_iter().map(convert_release_step_state).collect(),
            })
            .collect())
    }
}

fn map_status(status: tonic::Status) -> AuthError {
    match status.code() {
        tonic::Code::Unauthenticated => AuthError::InvalidCredentials,
        tonic::Code::AlreadyExists => AuthError::AlreadyExists(status.message().into()),
        tonic::Code::PermissionDenied => AuthError::PermissionDenied(status.message().into()),
        tonic::Code::Unavailable => AuthError::Unavailable(status.message().into()),
        tonic::Code::NotFound => AuthError::NotFound,
        // `FailedPrecondition` carries a wire-stable code in the message
        // for callers to branch on. Currently used by:
        //   - `last_auth_method` from `UnlinkOAuthProvider` (DATA-247)
        //   - `email_not_verified` from `Login` (handled inline above)
        // Unrecognised precondition codes fall through to `Other`.
        tonic::Code::FailedPrecondition if status.message() == "last_auth_method" => {
            AuthError::LastAuthMethod
        }
        _ => AuthError::Other(status.message().into()),
    }
}

fn convert_user(u: forage_grpc::User) -> User {
    User {
        user_id: u.user_id,
        username: u.username,
        profile_picture_url: u.profile_picture_url,
        mfa_enabled: u.mfa_enabled,
        emails: u
            .emails
            .into_iter()
            .map(|e| UserEmail {
                email: e.email,
                verified: e.verified,
            })
            .collect(),
    }
}

fn convert_token(t: forage_grpc::PersonalAccessToken) -> PersonalAccessToken {
    PersonalAccessToken {
        token_id: t.token_id,
        name: t.name,
        scopes: t.scopes,
        created_at: t.created_at.map(|ts| ts.to_string()),
        last_used: t.last_used.map(|ts| ts.to_string()),
        expires_at: t.expires_at.map(|ts| ts.to_string()),
    }
}

#[async_trait::async_trait]
impl ForestAuth for GrpcForestClient {
    #[tracing::instrument(skip_all)]
    async fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<RegisterResult, AuthError> {
        let resp = self
            .client()
            .register(forage_grpc::RegisterRequest {
                username: username.into(),
                email: email.into(),
                password: password.into(),
            })
            .await
            .map_err(map_status)?
            .into_inner();

        if resp.email_verification_required {
            return Ok(RegisterResult::VerificationRequired);
        }

        let tokens = resp.tokens.ok_or(AuthError::Other("no tokens in response".into()))?;
        Ok(RegisterResult::Success(AuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in_seconds: tokens.expires_in_seconds,
        }))
    }

    #[tracing::instrument(skip_all)]
    async fn login(&self, identifier: &str, password: &str) -> Result<LoginResult, AuthError> {
        let login_identifier = if identifier.contains('@') {
            forage_grpc::login_request::Identifier::Email(identifier.into())
        } else {
            forage_grpc::login_request::Identifier::Username(identifier.into())
        };

        let resp = match self
            .client()
            .login(forage_grpc::LoginRequest {
                identifier: Some(login_identifier),
                password: password.into(),
            })
            .await
        {
            Ok(resp) => resp.into_inner(),
            Err(status)
                if status.code() == tonic::Code::FailedPrecondition
                    && status.message() == "email_not_verified" =>
            {
                return Ok(LoginResult::EmailNotVerified);
            }
            Err(status) => return Err(map_status(status)),
        };

        if resp.mfa_required {
            return Ok(LoginResult::MfaRequired {
                mfa_session_token: resp.mfa_session_token,
            });
        }

        let tokens = resp.tokens.ok_or(AuthError::Other("no tokens in response".into()))?;
        Ok(LoginResult::Success(AuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in_seconds: tokens.expires_in_seconds,
        }))
    }

    #[tracing::instrument(skip_all)]
    async fn verify_login_mfa(
        &self,
        mfa_session_token: &str,
        code: &str,
    ) -> Result<AuthTokens, AuthError> {
        let resp = self
            .client()
            .verify_login_mfa(forage_grpc::VerifyLoginMfaRequest {
                mfa_session_token: mfa_session_token.into(),
                code: code.into(),
            })
            .await
            .map_err(map_status)?
            .into_inner();

        let tokens = resp.tokens.ok_or(AuthError::Other("no tokens in MFA response".into()))?;
        Ok(AuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in_seconds: tokens.expires_in_seconds,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn setup_mfa(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<MfaSetup, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::SetupMfaRequest {
                user_id: user_id.into(),
                mfa_type: forage_grpc::MfaType::Totp as i32,
            },
        )?;

        let resp = self
            .client()
            .setup_mfa(req)
            .await
            .map_err(map_status)?
            .into_inner();

        Ok(MfaSetup {
            mfa_id: resp.mfa_id,
            provisioning_uri: resp.provisioning_uri,
            secret: resp.secret,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn verify_mfa_setup(
        &self,
        access_token: &str,
        mfa_id: &str,
        code: &str,
    ) -> Result<(), AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::VerifyMfaRequest {
                mfa_id: mfa_id.into(),
                code: code.into(),
            },
        )?;

        self.client()
            .verify_mfa(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn disable_mfa(
        &self,
        access_token: &str,
        user_id: &str,
        code: &str,
    ) -> Result<(), AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::DisableMfaRequest {
                user_id: user_id.into(),
                code: code.into(),
            },
        )?;

        self.client()
            .disable_mfa(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn refresh_token(&self, refresh_token: &str) -> Result<AuthTokens, AuthError> {
        let resp = self
            .client()
            .refresh_token(forage_grpc::RefreshTokenRequest {
                refresh_token: refresh_token.into(),
            })
            .await
            .map_err(map_status)?
            .into_inner();

        let tokens = resp
            .tokens
            .ok_or(AuthError::Other("no tokens in response".into()))?;
        Ok(AuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in_seconds: tokens.expires_in_seconds,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn logout(&self, refresh_token: &str) -> Result<(), AuthError> {
        self.client()
            .logout(forage_grpc::LogoutRequest {
                refresh_token: refresh_token.into(),
            })
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn get_user(&self, access_token: &str) -> Result<User, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::TokenInfoRequest {},
        )?;

        let info = self
            .client()
            .token_info(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let req = Self::authed_request(
            access_token,
            forage_grpc::GetUserRequest {
                identifier: Some(forage_grpc::get_user_request::Identifier::UserId(
                    info.user_id,
                )),
            },
        )?;

        let resp = self
            .client()
            .get_user(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp.user.ok_or(AuthError::Other("no user in response".into()))?;
        Ok(convert_user(user))
    }

    #[tracing::instrument(skip_all)]
    async fn get_user_by_username(
        &self,
        access_token: &str,
        username: &str,
    ) -> Result<UserProfile, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::GetUserRequest {
                identifier: Some(forage_grpc::get_user_request::Identifier::Username(
                    username.into(),
                )),
            },
        )?;

        let resp = self
            .client()
            .get_user(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp
            .user
            .ok_or(AuthError::Other("no user in response".into()))?;
        Ok(UserProfile {
            user_id: user.user_id,
            username: user.username.clone(),
            profile_picture_url: user.profile_picture_url,
            created_at: user.created_at.map(|ts| {
                chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
        })
    }

    async fn get_user_by_email(
        &self,
        access_token: &str,
        email: &str,
    ) -> Result<UserProfile, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::GetUserRequest {
                identifier: Some(forage_grpc::get_user_request::Identifier::Email(
                    email.into(),
                )),
            },
        )?;

        let resp = self
            .client()
            .get_user(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp
            .user
            .ok_or(AuthError::Other("no user in response".into()))?;
        Ok(UserProfile {
            user_id: user.user_id,
            username: user.username.clone(),
            profile_picture_url: user.profile_picture_url,
            created_at: user.created_at.map(|ts| {
                chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
        })
    }

    #[tracing::instrument(skip_all)]
    async fn list_tokens(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<Vec<PersonalAccessToken>, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::ListPersonalAccessTokensRequest {
                user_id: user_id.into(),
            },
        )?;

        let resp = self
            .client()
            .list_personal_access_tokens(req)
            .await
            .map_err(map_status)?
            .into_inner();

        Ok(resp.tokens.into_iter().map(convert_token).collect())
    }

    #[tracing::instrument(skip_all)]
    async fn create_token(
        &self,
        access_token: &str,
        user_id: &str,
        name: &str,
    ) -> Result<CreatedToken, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::CreatePersonalAccessTokenRequest {
                user_id: user_id.into(),
                name: name.into(),
                scopes: vec![],
                expires_in_seconds: 0,
            },
        )?;

        let resp = self
            .client()
            .create_personal_access_token(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let token = resp
            .token
            .ok_or(AuthError::Other("no token in response".into()))?;
        Ok(CreatedToken {
            token: convert_token(token),
            raw_token: resp.raw_token,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn delete_token(
        &self,
        access_token: &str,
        token_id: &str,
    ) -> Result<(), AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::DeletePersonalAccessTokenRequest {
                token_id: token_id.into(),
            },
        )?;

        self.client()
            .delete_personal_access_token(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn update_username(
        &self,
        access_token: &str,
        user_id: &str,
        new_username: &str,
    ) -> Result<User, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::UpdateUserRequest {
                user_id: user_id.into(),
                username: Some(new_username.into()),
                profile_picture_url: None,
            },
        )?;

        let resp = self
            .client()
            .update_user(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp.user.ok_or(AuthError::Other("no user in response".into()))?;
        Ok(convert_user(user))
    }

    #[tracing::instrument(skip_all)]
    async fn update_profile_picture_url(
        &self,
        access_token: &str,
        user_id: &str,
        profile_picture_url: Option<&str>,
    ) -> Result<User, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::UpdateUserRequest {
                user_id: user_id.into(),
                username: None,
                profile_picture_url: profile_picture_url.map(|s| s.to_string()),
            },
        )?;

        let resp = self
            .client()
            .update_user(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp.user.ok_or(AuthError::Other("no user in response".into()))?;
        Ok(convert_user(user))
    }

    #[tracing::instrument(skip_all)]
    async fn change_password(
        &self,
        access_token: &str,
        user_id: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::ChangePasswordRequest {
                user_id: user_id.into(),
                current_password: current_password.into(),
                new_password: new_password.into(),
            },
        )?;

        self.client()
            .change_password(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn add_email(
        &self,
        access_token: &str,
        user_id: &str,
        email: &str,
    ) -> Result<AddEmailResult, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::AddEmailRequest {
                user_id: user_id.into(),
                email: email.into(),
            },
        )?;

        let resp = self
            .client()
            .add_email(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let email = resp.email.ok_or(AuthError::Other("no email in response".into()))?;
        Ok(AddEmailResult {
            email: UserEmail {
                email: email.email,
                verified: email.verified,
            },
            email_verification_required: resp.email_verification_required,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn confirm_email_verification(&self, email: &str) -> Result<(), AuthError> {
        let service_key = self
            .service_account_key
            .as_deref()
            .ok_or(AuthError::Other("service account key not configured".into()))?;

        let req = bearer_request(
            service_key,
            forage_grpc::ConfirmEmailVerificationRequest {
                email: email.into(),
            },
        )
        .map_err(AuthError::Other)?;

        self.client()
            .confirm_email_verification(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn remove_email(
        &self,
        access_token: &str,
        user_id: &str,
        email: &str,
    ) -> Result<(), AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::RemoveEmailRequest {
                user_id: user_id.into(),
                email: email.into(),
            },
        )?;

        self.client()
            .remove_email(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn oauth_login(
        &self,
        provider: &str,
        provider_user_id: &str,
        provider_email: &str,
        provider_display_name: &str,
        picture_url: Option<&str>,
    ) -> Result<forage_core::auth::OAuthLoginResult, AuthError> {
        let provider_enum = match provider {
            "google" => forage_grpc::OAuthProvider::OauthProviderGoogle as i32,
            "github" => forage_grpc::OAuthProvider::OauthProviderGithub as i32,
            "gitlab" => forage_grpc::OAuthProvider::OauthProviderGitlab as i32,
            "microsoft" => forage_grpc::OAuthProvider::OauthProviderMicrosoft as i32,
            "magic-link" => forage_grpc::OAuthProvider::OauthProviderMagicLink as i32,
            _ => return Err(AuthError::Other(format!("unsupported OAuth provider: {provider}"))),
        };

        // Use service account key for this privileged call.
        let service_key = self
            .service_account_key
            .as_deref()
            .ok_or(AuthError::Other("service account key not configured".into()))?;

        let req = bearer_request(
            service_key,
            forage_grpc::OAuthLoginRequest {
                provider: provider_enum,
                provider_user_id: provider_user_id.into(),
                provider_email: provider_email.into(),
                provider_display_name: provider_display_name.into(),
                provider_data_json: match picture_url {
                    Some(url) if url.starts_with("https://") => {
                        serde_json::json!({"picture_url": url}).to_string()
                    }
                    _ => String::new(),
                },
            },
        )
        .map_err(AuthError::Other)?;

        let resp = self
            .client()
            .o_auth_login(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp
            .user
            .map(convert_user)
            .ok_or(AuthError::Other("no user in OAuth response".into()))?;
        let tokens_proto = resp
            .tokens
            .ok_or(AuthError::Other("no tokens in OAuth response".into()))?;

        Ok(forage_core::auth::OAuthLoginResult {
            user,
            tokens: AuthTokens {
                access_token: tokens_proto.access_token,
                refresh_token: tokens_proto.refresh_token,
                expires_in_seconds: tokens_proto.expires_in_seconds,
            },
            is_new_user: resp.is_new_user,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn list_linked_identities(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<Vec<forage_core::auth::LinkedIdentity>, AuthError> {
        let req = Self::authed_request(
            access_token,
            forage_grpc::GetUserRequest {
                identifier: Some(forage_grpc::get_user_request::Identifier::UserId(
                    user_id.into(),
                )),
            },
        )?;

        let resp = self
            .client()
            .get_user(req)
            .await
            .map_err(map_status)?
            .into_inner();

        let user = resp.user.ok_or(AuthError::Other("no user in response".into()))?;

        let identities = user
            .oauth_connections
            .into_iter()
            .filter_map(convert_oauth_connection_to_linked)
            .collect();

        Ok(identities)
    }

    #[tracing::instrument(skip_all)]
    async fn link_oauth_provider(
        &self,
        access_token: &str,
        user_id: &str,
        input: &forage_core::auth::LinkOAuthInput,
    ) -> Result<(), AuthError> {
        let provider_enum = linked_provider_to_proto(input.provider)
            .ok_or_else(|| AuthError::Other("unsupported provider".into()))?;

        let req = Self::authed_request(
            access_token,
            forage_grpc::LinkOAuthProviderRequest {
                user_id: user_id.into(),
                provider: provider_enum,
                provider_user_id: input.provider_user_id.clone(),
                provider_email: input.provider_email.clone(),
                provider_display_name: input.provider_display_name.clone(),
                provider_data_json: input.provider_data_json.clone(),
            },
        )?;

        self.client()
            .link_o_auth_provider(req)
            .await
            .map_err(map_status)?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn unlink_oauth_provider(
        &self,
        access_token: &str,
        user_id: &str,
        provider: forage_core::auth::LinkedProvider,
    ) -> Result<(), AuthError> {
        let provider_enum = linked_provider_to_proto(provider)
            .ok_or_else(|| AuthError::Other("unsupported provider".into()))?;

        let req = Self::authed_request(
            access_token,
            forage_grpc::UnlinkOAuthProviderRequest {
                user_id: user_id.into(),
                provider: provider_enum,
            },
        )?;

        self.client()
            .unlink_o_auth_provider(req)
            .await
            .map_err(map_status)?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn approve_device_login(
        &self,
        user_code: &str,
        user_id: &str,
        approving_ip: &str,
        approving_user_agent: &str,
    ) -> Result<(), AuthError> {
        let service_key = self
            .service_account_key
            .as_deref()
            .ok_or(AuthError::Other("service account key not configured".into()))?;

        let req = bearer_request(
            service_key,
            forage_grpc::ApproveDeviceLoginRequest {
                user_code: user_code.into(),
                user_id: user_id.into(),
                approving_ip: approving_ip.into(),
                approving_user_agent: approving_user_agent.into(),
            },
        )
        .map_err(AuthError::Other)?;

        self.client()
            .approve_device_login(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn deny_device_login(
        &self,
        user_code: &str,
        user_id: &str,
    ) -> Result<(), AuthError> {
        let service_key = self
            .service_account_key
            .as_deref()
            .ok_or(AuthError::Other("service account key not configured".into()))?;

        let req = bearer_request(
            service_key,
            forage_grpc::DenyDeviceLoginRequest {
                user_code: user_code.into(),
                user_id: user_id.into(),
            },
        )
        .map_err(AuthError::Other)?;

        self.client()
            .deny_device_login(req)
            .await
            .map_err(map_status)?;
        Ok(())
    }
}

/// Map a forage-core `LinkedProvider` into the Forest proto enum value.
/// Returns `None` for `Slack`, which Forest does not know about.
fn linked_provider_to_proto(p: forage_core::auth::LinkedProvider) -> Option<i32> {
    match p {
        forage_core::auth::LinkedProvider::GitHub => {
            Some(forage_grpc::OAuthProvider::OauthProviderGithub as i32)
        }
        forage_core::auth::LinkedProvider::Google => {
            Some(forage_grpc::OAuthProvider::OauthProviderGoogle as i32)
        }
        forage_core::auth::LinkedProvider::Slack => None,
    }
}

/// Map a Forest `OAuthConnection` into a forage-core `LinkedIdentity`.
/// Returns `None` for providers we don't surface on the UI
/// (e.g. `MAGIC_LINK`).
fn convert_oauth_connection_to_linked(
    c: forage_grpc::OAuthConnection,
) -> Option<forage_core::auth::LinkedIdentity> {
    use forage_grpc::OAuthProvider as P;
    let provider = match P::try_from(c.provider).ok()? {
        P::OauthProviderGithub => forage_core::auth::LinkedProvider::GitHub,
        P::OauthProviderGoogle => forage_core::auth::LinkedProvider::Google,
        _ => return None,
    };
    let linked_at = c.linked_at.as_ref().and_then(|ts| {
        chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32).map(|dt| dt.to_rfc3339())
    });
    let provider_email = if c.provider_email.is_empty() {
        None
    } else {
        Some(c.provider_email.as_str())
    };
    // Decode the provider-specific extras Forest stored at link time.
    // Both fields are best-effort — fall back to `None` if missing or
    // malformed so the link still renders.
    let display_name_override = if c.provider_display_name.is_empty() {
        None
    } else {
        Some(c.provider_display_name.clone())
    };
    let extras: Option<forage_core::auth::ProviderDataExtras> = if c.provider_data_json.is_empty() {
        None
    } else {
        serde_json::from_str(&c.provider_data_json).ok()
    };
    let mut identity = forage_core::auth::linked_identity_from_forest(
        provider,
        &c.provider_user_id,
        provider_email,
        linked_at.as_deref(),
        extras.as_ref(),
    );
    // The server-side helper splits `display_name` out of the JSON and
    // sends it via the dedicated field — prefer it when present, since
    // it's the value the link flow explicitly wrote.
    if let Some(name) = display_name_override {
        identity.display_name = name;
    }
    Some(identity)
}

fn convert_organisations(
    organisations: Vec<forage_grpc::Organisation>,
    roles: Vec<String>,
) -> Vec<Organisation> {
    organisations
        .into_iter()
        .zip(roles)
        .map(|(org, role)| Organisation {
            organisation_id: org.organisation_id,
            name: org.name,
            role,
        })
        .collect()
}

fn convert_project(p: forage_grpc::Project) -> forage_core::platform::Project {
    forage_core::platform::Project {
        organisation: p.organisation,
        project: p.project,
        readme: p.readme,
        description: p.description,
        metadata: p
            .metadata
            .map(convert_project_metadata)
            .unwrap_or_default(),
    }
}

fn convert_project_metadata(
    m: forage_grpc::ProjectMetadata,
) -> forage_core::platform::ProjectMetadata {
    forage_core::platform::ProjectMetadata {
        git_url: m.git_url,
        homepage: m.homepage,
        docs_url: m.docs_url,
        support_url: m.support_url,
        domain: m.domain,
        owner: m.owner,
    }
}

fn convert_artifact(a: forage_grpc::Artifact) -> Artifact {
    let ctx = a.context.unwrap_or_default();
    let source = a.source.map(|s| ArtifactSource {
        user: s.user.filter(|v| !v.is_empty()),
        email: s.email.filter(|v| !v.is_empty()),
        source_type: s.source_type.filter(|v| !v.is_empty()),
        run_url: s.run_url.filter(|v| !v.is_empty()),
    });
    let git_ref = a.r#ref.map(|r| ArtifactRef {
        commit_sha: r.commit_sha,
        branch: r.branch.filter(|v| !v.is_empty()),
        commit_message: r.commit_message.filter(|v| !v.is_empty()),
        version: r.version.filter(|v| !v.is_empty()),
        repo_url: r.repo_url.filter(|v| !v.is_empty()),
    });
    let destinations = a
        .destinations
        .into_iter()
        .map(|d| ArtifactDestination {
            name: d.name,
            environment: d.environment,
            type_organisation: if d.type_organisation.is_empty() {
                None
            } else {
                Some(d.type_organisation)
            },
            type_name: if d.type_name.is_empty() {
                None
            } else {
                Some(d.type_name)
            },
            type_version: if d.type_version == 0 {
                None
            } else {
                Some(d.type_version)
            },
            status: if d.status.is_empty() {
                None
            } else {
                Some(d.status)
            },
        })
        .collect();
    Artifact {
        artifact_id: a.artifact_id,
        slug: a.slug,
        context: ArtifactContext {
            title: ctx.title,
            description: if ctx.description.as_deref() == Some("") {
                None
            } else {
                ctx.description
            },
            web: ctx.web.filter(|v| !v.is_empty()),
            pr: ctx.pr.filter(|v| !v.is_empty()),
        },
        source,
        git_ref,
        destinations,
        created_at: a.created_at,
    }
}

fn convert_pipeline_stage(s: forage_grpc::PipelineStage) -> PipelineStage {
    let config = match s.config {
        Some(forage_grpc::pipeline_stage::Config::Deploy(d)) => {
            PipelineStageConfig::Deploy { environment: d.environment }
        }
        Some(forage_grpc::pipeline_stage::Config::Wait(w)) => {
            PipelineStageConfig::Wait { duration_seconds: w.duration_seconds }
        }
        Some(forage_grpc::pipeline_stage::Config::Plan(p)) => {
            PipelineStageConfig::Plan { environment: p.environment, auto_approve: p.auto_approve }
        }
        None => PipelineStageConfig::Deploy { environment: String::new() },
    };
    PipelineStage {
        id: s.id,
        depends_on: s.depends_on,
        config,
    }
}

/// Convert a `PipelineStageState` proto message (from GetReleaseIntentStates)
/// to the domain type. Same enum mapping as `convert_pipeline_run_stage`.
fn convert_pipeline_stage_state(
    s: forage_grpc::PipelineStageState,
) -> forage_core::platform::PipelineRunStageState {
    let stage_type = match forage_grpc::PipelineRunStageType::try_from(s.stage_type) {
        Ok(forage_grpc::PipelineRunStageType::Deploy) => "deploy",
        Ok(forage_grpc::PipelineRunStageType::Wait) => "wait",
        Ok(forage_grpc::PipelineRunStageType::Plan) => "plan",
        _ => "unknown",
    };
    let status = match forage_grpc::PipelineRunStageStatus::try_from(s.status) {
        Ok(forage_grpc::PipelineRunStageStatus::Pending) => "PENDING",
        Ok(forage_grpc::PipelineRunStageStatus::Active) => "RUNNING",
        Ok(forage_grpc::PipelineRunStageStatus::Succeeded) => "SUCCEEDED",
        Ok(forage_grpc::PipelineRunStageStatus::Failed) => "FAILED",
        Ok(forage_grpc::PipelineRunStageStatus::Cancelled) => "CANCELLED",
        Ok(forage_grpc::PipelineRunStageStatus::AwaitingApproval) => "AWAITING_APPROVAL",
        _ => "PENDING",
    };
    forage_core::platform::PipelineRunStageState {
        stage_id: s.stage_id,
        depends_on: s.depends_on,
        stage_type: stage_type.into(),
        status: status.into(),
        environment: s.environment,
        duration_seconds: s.duration_seconds,
        queued_at: s.queued_at,
        started_at: s.started_at,
        completed_at: s.completed_at,
        error_message: s.error_message,
        wait_until: s.wait_until,
        release_ids: s.release_ids,
        approval_status: s.approval_status,
        auto_approve: s.auto_approve,
    }
}

fn convert_release_step_state(
    s: forage_grpc::ReleaseStepState,
) -> forage_core::platform::ReleaseStepState {
    forage_core::platform::ReleaseStepState {
        release_id: s.release_id,
        stage_id: s.stage_id,
        destination_name: s.destination_name,
        environment: s.environment,
        status: s.status,
        queued_at: s.queued_at,
        assigned_at: s.assigned_at,
        started_at: s.started_at,
        completed_at: s.completed_at,
        error_message: s.error_message,
    }
}

fn convert_stages_to_grpc(stages: &[PipelineStage]) -> Vec<forage_grpc::PipelineStage> {
    stages
        .iter()
        .map(|s| forage_grpc::PipelineStage {
            id: s.id.clone(),
            depends_on: s.depends_on.clone(),
            config: Some(match &s.config {
                PipelineStageConfig::Deploy { environment } => {
                    forage_grpc::pipeline_stage::Config::Deploy(forage_grpc::DeployStageConfig {
                        environment: environment.clone(),
                    })
                }
                PipelineStageConfig::Wait { duration_seconds } => {
                    forage_grpc::pipeline_stage::Config::Wait(forage_grpc::WaitStageConfig {
                        duration_seconds: *duration_seconds,
                    })
                }
                PipelineStageConfig::Plan { environment, auto_approve } => {
                    forage_grpc::pipeline_stage::Config::Plan(forage_grpc::PlanStageConfig {
                        environment: environment.clone(),
                        auto_approve: *auto_approve,
                    })
                }
            }),
        })
        .collect()
}

fn convert_release_pipeline(p: forage_grpc::ReleasePipeline) -> ReleasePipeline {
    ReleasePipeline {
        id: p.id,
        name: p.name,
        enabled: p.enabled,
        stages: p.stages.into_iter().map(convert_pipeline_stage).collect(),
        created_at: p.created_at,
        updated_at: p.updated_at,
    }
}

fn convert_trigger(t: forage_grpc::Trigger) -> Trigger {
    Trigger {
        id: t.id,
        name: t.name,
        enabled: t.enabled,
        branch_pattern: t.branch_pattern,
        title_pattern: t.title_pattern,
        author_pattern: t.author_pattern,
        commit_message_pattern: t.commit_message_pattern,
        source_type_pattern: t.source_type_pattern,
        target_environments: t.target_environments,
        target_destinations: t.target_destinations,
        force_release: t.force_release,
        use_pipeline: t.use_pipeline,
        created_at: t.created_at,
        updated_at: t.updated_at,
    }
}

fn convert_policy(p: forage_grpc::Policy) -> Policy {
    let policy_type_str = match forage_grpc::PolicyType::try_from(p.policy_type) {
        Ok(forage_grpc::PolicyType::SoakTime) => "soak_time",
        Ok(forage_grpc::PolicyType::BranchRestriction) => "branch_restriction",
        Ok(forage_grpc::PolicyType::ExternalApproval) => "approval",
        _ => "unknown",
    };
    let config = match p.config {
        Some(forage_grpc::policy::Config::SoakTime(c)) => PolicyConfig::SoakTime {
            source_environment: c.source_environment,
            target_environment: c.target_environment,
            duration_seconds: c.duration_seconds,
        },
        Some(forage_grpc::policy::Config::BranchRestriction(c)) => {
            PolicyConfig::BranchRestriction {
                target_environment: c.target_environment,
                branch_pattern: c.branch_pattern,
            }
        }
        Some(forage_grpc::policy::Config::ExternalApproval(c)) => PolicyConfig::Approval {
            target_environment: c.target_environment,
            required_approvals: c.required_approvals,
        },
        None => PolicyConfig::SoakTime {
            source_environment: String::new(),
            target_environment: String::new(),
            duration_seconds: 0,
        },
    };
    Policy {
        id: p.id,
        name: p.name,
        enabled: p.enabled,
        policy_type: policy_type_str.into(),
        config,
        created_at: p.created_at,
        updated_at: p.updated_at,
    }
}

fn policy_config_to_grpc(
    config: &PolicyConfig,
) -> (i32, Option<forage_grpc::create_policy_request::Config>) {
    match config {
        PolicyConfig::SoakTime {
            source_environment,
            target_environment,
            duration_seconds,
        } => (
            forage_grpc::PolicyType::SoakTime as i32,
            Some(forage_grpc::create_policy_request::Config::SoakTime(
                forage_grpc::SoakTimeConfig {
                    source_environment: source_environment.clone(),
                    target_environment: target_environment.clone(),
                    duration_seconds: *duration_seconds,
                },
            )),
        ),
        PolicyConfig::BranchRestriction {
            target_environment,
            branch_pattern,
        } => (
            forage_grpc::PolicyType::BranchRestriction as i32,
            Some(
                forage_grpc::create_policy_request::Config::BranchRestriction(
                    forage_grpc::BranchRestrictionConfig {
                        target_environment: target_environment.clone(),
                        branch_pattern: branch_pattern.clone(),
                    },
                ),
            ),
        ),
        PolicyConfig::Approval {
            target_environment,
            required_approvals,
        } => (
            forage_grpc::PolicyType::ExternalApproval as i32,
            Some(
                forage_grpc::create_policy_request::Config::ExternalApproval(
                    forage_grpc::ExternalApprovalConfig {
                        target_environment: target_environment.clone(),
                        required_approvals: *required_approvals,
                    },
                ),
            ),
        ),
    }
}

fn convert_member(m: forage_grpc::OrganisationMember) -> OrgMember {
    OrgMember {
        user_id: m.user_id,
        username: m.username,
        role: m.role,
        joined_at: m.joined_at.map(|ts| ts.to_string()),
    }
}

fn map_platform_status(status: tonic::Status) -> PlatformError {
    match status.code() {
        tonic::Code::Unauthenticated => PlatformError::NotAuthenticated,
        tonic::Code::PermissionDenied => {
            PlatformError::Other(status.message().into())
        }
        tonic::Code::NotFound => PlatformError::NotFound(status.message().into()),
        tonic::Code::Unavailable => PlatformError::Unavailable(status.message().into()),
        _ => PlatformError::Other(status.message().into()),
    }
}

fn platform_authed_request<T>(access_token: &str, msg: T) -> Result<Request<T>, PlatformError> {
    if access_token.is_empty() {
        return Ok(Request::new(msg));
    }
    bearer_request(access_token, msg).map_err(PlatformError::Other)
}

#[async_trait::async_trait]
impl ForestPlatform for GrpcForestClient {
    #[tracing::instrument(skip_all)]
    async fn list_my_organisations(
        &self,
        access_token: &str,
    ) -> Result<Vec<Organisation>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListMyOrganisationsRequest { role: String::new() },
        )?;

        let resp = self
            .org_client()
            .list_my_organisations(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        Ok(convert_organisations(resp.organisations, resp.roles))
    }

    #[tracing::instrument(skip_all)]
    async fn get_project(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Option<forage_core::platform::Project>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetProjectRequest {
                organisation: organisation.into(),
                project: project.into(),
            },
        )?;
        let resp = match self.release_client().get_project(req).await {
            Ok(r) => r.into_inner(),
            Err(status) if status.code() == tonic::Code::NotFound => return Ok(None),
            Err(status) => return Err(map_platform_status(status)),
        };
        let proto = match resp.project {
            Some(p) => p,
            None => return Ok(None),
        };
        Ok(Some(convert_project(proto)))
    }

    #[tracing::instrument(skip_all)]
    async fn list_projects(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<String>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetProjectsRequest {
                query: Some(forage_grpc::get_projects_request::Query::Organisation(
                    forage_grpc::OrganisationRef {
                        organisation: organisation.into(),
                    },
                )),
            },
        )?;

        let resp = self
            .release_client()
            .get_projects(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        Ok(resp.projects)
    }

    #[tracing::instrument(skip_all)]
    async fn list_artifacts(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Artifact>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetArtifactsByProjectRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
            },
        )?;

        let resp = self
            .release_client()
            .get_artifacts_by_project(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        Ok(resp.artifact.into_iter().map(convert_artifact).collect())
    }

    async fn create_organisation(
        &self,
        access_token: &str,
        name: &str,
    ) -> Result<String, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::CreateOrganisationRequest {
                name: name.into(),
            },
        )?;

        let resp = self
            .org_client()
            .create_organisation(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        Ok(resp.organisation_id)
    }

    #[tracing::instrument(skip_all)]
    async fn list_members(
        &self,
        access_token: &str,
        organisation_id: &str,
    ) -> Result<Vec<OrgMember>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListMembersRequest {
                organisation_id: organisation_id.into(),
                page_size: 100,
                page_token: String::new(),
            },
        )?;

        let resp = self
            .org_client()
            .list_members(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        Ok(resp.members.into_iter().map(convert_member).collect())
    }

    async fn add_member(
        &self,
        access_token: &str,
        organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<OrgMember, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::AddMemberRequest {
                organisation_id: organisation_id.into(),
                user_id: user_id.into(),
                role: role.into(),
            },
        )?;

        let resp = self
            .org_client()
            .add_member(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        let member = resp
            .member
            .ok_or(PlatformError::Other("no member in response".into()))?;
        Ok(convert_member(member))
    }

    async fn remove_member(
        &self,
        access_token: &str,
        organisation_id: &str,
        user_id: &str,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::RemoveMemberRequest {
                organisation_id: organisation_id.into(),
                user_id: user_id.into(),
            },
        )?;

        self.org_client()
            .remove_member(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    async fn update_member_role(
        &self,
        access_token: &str,
        organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<OrgMember, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::UpdateMemberRoleRequest {
                organisation_id: organisation_id.into(),
                user_id: user_id.into(),
                role: role.into(),
            },
        )?;

        let resp = self
            .org_client()
            .update_member_role(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        let member = resp
            .member
            .ok_or(PlatformError::Other("no member in response".into()))?;
        Ok(convert_member(member))
    }

    #[tracing::instrument(skip_all)]
    async fn get_artifact_by_slug(
        &self,
        access_token: &str,
        slug: &str,
    ) -> Result<Artifact, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetArtifactBySlugRequest {
                slug: slug.into(),
            },
        )?;

        let resp = self
            .release_client()
            .get_artifact_by_slug(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        let artifact = resp
            .artifact
            .ok_or(PlatformError::NotFound("artifact not found".into()))?;
        Ok(convert_artifact(artifact))
    }

    #[tracing::instrument(skip_all)]
    async fn list_environments(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<Environment>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListEnvironmentsRequest {
                organisation: organisation.into(),
            },
        )?;
        let resp = self
            .env_client()
            .list_environments(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp
            .environments
            .into_iter()
            .map(|e| Environment {
                id: e.id,
                organisation: e.organisation,
                name: e.name,
                description: e.description.filter(|v| !v.is_empty()),
                sort_order: e.sort_order,
                created_at: e.created_at,
            })
            .collect())
    }

    #[tracing::instrument(skip_all)]
    async fn list_destinations(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<Destination>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetDestinationsRequest {
                organisation: organisation.into(),
            },
        )?;
        let resp = self
            .dest_client()
            .get_destinations(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp
            .destinations
            .into_iter()
            .map(|d| Destination {
                name: d.name,
                environment: d.environment,
                organisation: d.organisation,
                metadata: d.metadata,
                dest_type: d.r#type.map(|t| DestinationType {
                    organisation: t.organisation,
                    name: t.name,
                    version: t.version,
                }),
            })
            .collect())
    }

    async fn create_environment(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        description: Option<&str>,
        sort_order: i32,
    ) -> Result<Environment, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::CreateEnvironmentRequest {
                organisation: organisation.into(),
                name: name.into(),
                description: description.map(|s| s.to_string()),
                sort_order,
            },
        )?;
        let resp = self
            .env_client()
            .create_environment(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let e = resp
            .environment
            .ok_or(PlatformError::Other("no environment in response".into()))?;
        Ok(Environment {
            id: e.id,
            organisation: e.organisation,
            name: e.name,
            description: e.description.filter(|v| !v.is_empty()),
            sort_order: e.sort_order,
            created_at: e.created_at,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn update_environment(
        &self,
        access_token: &str,
        id: &str,
        description: Option<&str>,
        sort_order: Option<i32>,
    ) -> Result<Environment, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::UpdateEnvironmentRequest {
                id: id.into(),
                description: description.map(|s| s.to_string()),
                sort_order,
            },
        )?;
        let resp = self
            .env_client()
            .update_environment(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let e = resp
            .environment
            .ok_or(PlatformError::Other("no environment in response".into()))?;
        Ok(Environment {
            id: e.id,
            organisation: e.organisation,
            name: e.name,
            description: e.description.filter(|v| !v.is_empty()),
            sort_order: e.sort_order,
            created_at: e.created_at,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn create_destination(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        environment: &str,
        metadata: &std::collections::HashMap<String, String>,
        dest_type: Option<&forage_core::platform::DestinationType>,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::CreateDestinationRequest {
                organisation: organisation.into(),
                name: name.into(),
                environment: environment.into(),
                metadata: metadata.clone(),
                r#type: dest_type.map(|t| forage_grpc::DestinationType {
                    organisation: t.organisation.clone(),
                    name: t.name.clone(),
                    version: t.version,
                    description: String::new(),
                    fields: vec![],
                }),
            },
        )?;
        self.dest_client()
            .create_destination(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn list_destination_types(
        &self,
        access_token: &str,
    ) -> Result<Vec<DestinationTypeInfo>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListDestinationTypesRequest {},
        )?;
        let resp = self
            .dest_client()
            .list_destination_types(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp
            .types
            .into_iter()
            .map(|t| DestinationTypeInfo {
                organisation: t.organisation,
                name: t.name,
                version: t.version,
                description: t.description,
                fields: t
                    .fields
                    .into_iter()
                    .map(|f| MetadataFieldDef {
                        name: f.name,
                        label: f.label,
                        description: f.description,
                        required: f.required,
                        field_type: f.field_type,
                        default_value: f.default_value,
                    })
                    .collect(),
            })
            .collect())
    }

    async fn update_destination(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        metadata: &std::collections::HashMap<String, String>,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::UpdateDestinationRequest {
                name: name.into(),
                metadata: metadata.clone(),
                organisation: organisation.into(),
            },
        )?;
        self.dest_client()
            .update_destination(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn get_destination_states(
        &self,
        access_token: &str,
        organisation: &str,
        project: Option<&str>,
    ) -> Result<forage_core::platform::DeploymentStates, PlatformError> {
        let req = bearer_request(
            access_token,
            forage_grpc::GetDestinationStatesRequest {
                organisation: organisation.into(),
                project: project.map(|p| p.into()),
            },
        )
        .map_err(|e| PlatformError::Other(e.to_string()))?;

        let resp = self
            .release_client()
            .get_destination_states(req)
            .await
            .map_err(map_platform_status)?;

        let inner = resp.into_inner();

        let destinations = inner
            .destinations
            .into_iter()
            .map(|d| forage_core::platform::DestinationState {
                destination_id: d.destination_id,
                destination_name: d.destination_name,
                environment: d.environment,
                release_id: d.release_id,
                artifact_id: d.artifact_id,
                status: d.status,
                error_message: d.error_message,
                queued_at: d.queued_at,
                completed_at: d.completed_at,
                queue_position: d.queue_position,
                started_at: d.started_at,
            })
            .collect();

        Ok(forage_core::platform::DeploymentStates {
            destinations,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn get_release_intent_states(
        &self,
        access_token: &str,
        organisation: &str,
        project: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<forage_core::platform::ReleaseIntentState>, PlatformError> {
        let req = bearer_request(
            access_token,
            forage_grpc::GetReleaseIntentStatesRequest {
                organisation: organisation.into(),
                project: project.map(|p| p.into()),
                include_completed,
            },
        )
        .map_err(|e| PlatformError::Other(e.to_string()))?;

        let resp = self
            .release_client()
            .get_release_intent_states(req)
            .await
            .map_err(map_platform_status)?;

        Ok(resp
            .into_inner()
            .release_intents
            .into_iter()
            .map(|ri| forage_core::platform::ReleaseIntentState {
                release_intent_id: ri.release_intent_id,
                artifact_id: ri.artifact_id,
                project: ri.project,
                created_at: ri.created_at,
                stages: ri
                    .stages
                    .into_iter()
                    .map(convert_pipeline_stage_state)
                    .collect(),
                steps: ri
                    .steps
                    .into_iter()
                    .map(convert_release_step_state)
                    .collect(),
            })
            .collect())
    }

    #[tracing::instrument(skip_all)]
    async fn release_artifact(
        &self,
        access_token: &str,
        artifact_id: &str,
        destinations: &[String],
        environments: &[String],
        use_pipeline: bool,
    ) -> Result<(), PlatformError> {
        let req = bearer_request(
            access_token,
            forage_grpc::ReleaseRequest {
                artifact_id: artifact_id.into(),
                destinations: destinations.to_vec(),
                environments: environments.to_vec(),
                force: false,
                use_pipeline,
                prepare_only: false,
            },
        )
        .map_err(|e| PlatformError::Other(e.to_string()))?;

        self.release_client()
            .release(req)
            .await
            .map_err(map_platform_status)?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn list_triggers(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Trigger>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListTriggersRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
            },
        )?;
        let resp = self
            .trigger_client()
            .list_triggers(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp.triggers.into_iter().map(convert_trigger).collect())
    }

    async fn create_trigger(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        input: &CreateTriggerInput,
    ) -> Result<Trigger, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::CreateTriggerRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: input.name.clone(),
                branch_pattern: input.branch_pattern.clone(),
                title_pattern: input.title_pattern.clone(),
                author_pattern: input.author_pattern.clone(),
                commit_message_pattern: input.commit_message_pattern.clone(),
                source_type_pattern: input.source_type_pattern.clone(),
                target_environments: input.target_environments.clone(),
                target_destinations: input.target_destinations.clone(),
                force_release: input.force_release,
                use_pipeline: input.use_pipeline,
            },
        )?;
        let resp = self
            .trigger_client()
            .create_trigger(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let trigger = resp
            .trigger
            .ok_or(PlatformError::Other("no trigger in response".into()))?;
        Ok(convert_trigger(trigger))
    }

    async fn update_trigger(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
        input: &UpdateTriggerInput,
    ) -> Result<Trigger, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::UpdateTriggerRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: name.into(),
                enabled: input.enabled,
                branch_pattern: input.branch_pattern.clone(),
                title_pattern: input.title_pattern.clone(),
                author_pattern: input.author_pattern.clone(),
                commit_message_pattern: input.commit_message_pattern.clone(),
                source_type_pattern: input.source_type_pattern.clone(),
                target_environments: input.target_environments.clone(),
                target_destinations: input.target_destinations.clone(),
                force_release: input.force_release,
                use_pipeline: input.use_pipeline,
            },
        )?;
        let resp = self
            .trigger_client()
            .update_trigger(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let trigger = resp
            .trigger
            .ok_or(PlatformError::Other("no trigger in response".into()))?;
        Ok(convert_trigger(trigger))
    }

    async fn delete_trigger(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::DeleteTriggerRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: name.into(),
            },
        )?;
        self.trigger_client()
            .delete_trigger(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    async fn list_policies(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Policy>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListPoliciesRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
            },
        )?;
        let resp = self
            .policy_client()
            .list_policies(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp.policies.into_iter().map(convert_policy).collect())
    }

    async fn create_policy(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        input: &CreatePolicyInput,
    ) -> Result<Policy, PlatformError> {
        let (policy_type, config) = policy_config_to_grpc(&input.config);
        let req = platform_authed_request(
            access_token,
            forage_grpc::CreatePolicyRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: input.name.clone(),
                policy_type,
                config,
            },
        )?;
        let resp = self
            .policy_client()
            .create_policy(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let policy = resp
            .policy
            .ok_or(PlatformError::Other("no policy in response".into()))?;
        Ok(convert_policy(policy))
    }

    async fn update_policy(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
        input: &UpdatePolicyInput,
    ) -> Result<Policy, PlatformError> {
        let config = input.config.as_ref().map(|c| {
            let (_, grpc_config) = policy_config_to_grpc(c);
            match grpc_config {
                Some(forage_grpc::create_policy_request::Config::SoakTime(s)) => {
                    forage_grpc::update_policy_request::Config::SoakTime(s)
                }
                Some(forage_grpc::create_policy_request::Config::BranchRestriction(b)) => {
                    forage_grpc::update_policy_request::Config::BranchRestriction(b)
                }
                Some(forage_grpc::create_policy_request::Config::ExternalApproval(a)) => {
                    forage_grpc::update_policy_request::Config::ExternalApproval(a)
                }
                None => forage_grpc::update_policy_request::Config::SoakTime(
                    forage_grpc::SoakTimeConfig::default(),
                ),
            }
        });
        let req = platform_authed_request(
            access_token,
            forage_grpc::UpdatePolicyRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: name.into(),
                enabled: input.enabled,
                config,
            },
        )?;
        let resp = self
            .policy_client()
            .update_policy(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let policy = resp
            .policy
            .ok_or(PlatformError::Other("no policy in response".into()))?;
        Ok(convert_policy(policy))
    }

    async fn delete_policy(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::DeletePolicyRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: name.into(),
            },
        )?;
        self.policy_client()
            .delete_policy(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn list_release_pipelines(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<ReleasePipeline>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListReleasePipelinesRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
            },
        )?;
        let resp = self
            .pipeline_client()
            .list_release_pipelines(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp
            .pipelines
            .into_iter()
            .map(convert_release_pipeline)
            .collect())
    }

    async fn create_release_pipeline(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        input: &CreateReleasePipelineInput,
    ) -> Result<ReleasePipeline, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::CreateReleasePipelineRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: input.name.clone(),
                stages: convert_stages_to_grpc(&input.stages),
            },
        )?;
        let resp = self
            .pipeline_client()
            .create_release_pipeline(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let pipeline = resp
            .pipeline
            .ok_or(PlatformError::Other("no pipeline in response".into()))?;
        Ok(convert_release_pipeline(pipeline))
    }

    async fn update_release_pipeline(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
        input: &UpdateReleasePipelineInput,
    ) -> Result<ReleasePipeline, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::UpdateReleasePipelineRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: name.into(),
                enabled: input.enabled,
                stages: input.stages.as_ref().map(|s| convert_stages_to_grpc(s)).unwrap_or_default(),
                update_stages: input.stages.is_some(),
            },
        )?;
        let resp = self
            .pipeline_client()
            .update_release_pipeline(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let pipeline = resp
            .pipeline
            .ok_or(PlatformError::Other("no pipeline in response".into()))?;
        Ok(convert_release_pipeline(pipeline))
    }

    async fn delete_release_pipeline(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::DeleteReleasePipelineRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: name.into(),
            },
        )?;
        self.pipeline_client()
            .delete_release_pipeline(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn get_artifact_spec(
        &self,
        access_token: &str,
        artifact_id: &str,
    ) -> Result<String, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetArtifactSpecRequest {
                artifact_id: artifact_id.into(),
            },
        )?;
        let resp = self
            .artifact_client()
            .get_artifact_spec(req)
            .await
            .map_err(map_platform_status)?;
        Ok(resp.into_inner().content)
    }

    async fn get_notification_preferences(
        &self,
        access_token: &str,
    ) -> Result<Vec<NotificationPreference>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetNotificationPreferencesRequest {},
        )?;
        let resp = self
            .notification_client()
            .get_notification_preferences(req)
            .await
            .map_err(map_platform_status)?;
        Ok(resp
            .into_inner()
            .preferences
            .into_iter()
            .map(|p| {
                let nt = forage_grpc::NotificationType::try_from(p.notification_type)
                    .unwrap_or(forage_grpc::NotificationType::Unspecified);
                let ch = forage_grpc::NotificationChannel::try_from(p.channel)
                    .unwrap_or(forage_grpc::NotificationChannel::Unspecified);
                NotificationPreference {
                    notification_type: nt.as_str_name().to_string(),
                    channel: ch.as_str_name().to_string(),
                    enabled: p.enabled,
                }
            })
            .collect())
    }

    async fn set_notification_preference(
        &self,
        access_token: &str,
        notification_type: &str,
        channel: &str,
        enabled: bool,
    ) -> Result<(), PlatformError> {
        let nt = forage_grpc::NotificationType::from_str_name(notification_type)
            .unwrap_or(forage_grpc::NotificationType::Unspecified) as i32;
        let ch = forage_grpc::NotificationChannel::from_str_name(channel)
            .unwrap_or(forage_grpc::NotificationChannel::Unspecified) as i32;
        let req = platform_authed_request(
            access_token,
            forage_grpc::SetNotificationPreferenceRequest {
                notification_type: nt,
                channel: ch,
                enabled,
            },
        )?;
        self.notification_client()
            .set_notification_preference(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn evaluate_policies(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        target_environment: &str,
        release_intent_id: Option<&str>,
    ) -> Result<Vec<PolicyEvaluation>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::EvaluatePoliciesRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                target_environment: target_environment.into(),
                branch: None,
                release_intent_id: release_intent_id.map(|s| s.to_string()),
            },
        )?;
        let resp = self
            .policy_client()
            .evaluate_policies(req)
            .await
            .map_err(map_platform_status)?;
        Ok(resp
            .into_inner()
            .evaluations
            .into_iter()
            .map(convert_policy_evaluation)
            .collect())
    }

    #[tracing::instrument(skip_all)]
    async fn approve_release(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
        target_environment: &str,
        comment: Option<&str>,
        force_bypass: bool,
    ) -> Result<ApprovalState, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ExternalApproveReleaseRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                release_intent_id: release_intent_id.into(),
                target_environment: target_environment.into(),
                comment: comment.map(|s| s.to_string()),
                force_bypass,
            },
        )?;
        let resp = self
            .policy_client()
            .external_approve_release(req)
            .await
            .map_err(map_platform_status)?;
        Ok(convert_approval_state(resp.into_inner().state))
    }

    #[tracing::instrument(skip_all)]
    async fn reject_release(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
        target_environment: &str,
        comment: Option<&str>,
    ) -> Result<ApprovalState, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ExternalRejectReleaseRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                release_intent_id: release_intent_id.into(),
                target_environment: target_environment.into(),
                comment: comment.map(|s| s.to_string()),
            },
        )?;
        let resp = self
            .policy_client()
            .external_reject_release(req)
            .await
            .map_err(map_platform_status)?;
        Ok(convert_approval_state(resp.into_inner().state))
    }

    #[tracing::instrument(skip_all)]
    async fn get_approval_state(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
        target_environment: &str,
    ) -> Result<ApprovalState, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetExternalApprovalStateRequest {
                project: Some(forage_grpc::Project {
                    organisation: organisation.into(),
                    project: project.into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                release_intent_id: release_intent_id.into(),
                target_environment: target_environment.into(),
            },
        )?;
        let resp = self
            .policy_client()
            .get_external_approval_state(req)
            .await
            .map_err(map_platform_status)?;
        Ok(convert_approval_state(resp.into_inner().state))
    }

    async fn approve_plan_stage(
        &self,
        access_token: &str,
        release_intent_id: &str,
        stage_id: &str,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ApprovePlanStageRequest {
                release_intent_id: release_intent_id.into(),
                stage_id: stage_id.into(),
            },
        )?;
        self.release_client()
            .approve_plan_stage(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    async fn reject_plan_stage(
        &self,
        access_token: &str,
        release_intent_id: &str,
        stage_id: &str,
        reason: Option<&str>,
    ) -> Result<(), PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::RejectPlanStageRequest {
                release_intent_id: release_intent_id.into(),
                stage_id: stage_id.into(),
                reason: reason.map(|s| s.into()),
            },
        )?;
        self.release_client()
            .reject_plan_stage(req)
            .await
            .map_err(map_platform_status)?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn get_plan_output(
        &self,
        access_token: &str,
        release_intent_id: &str,
        stage_id: &str,
    ) -> Result<PlanOutput, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetPlanOutputRequest {
                release_intent_id: release_intent_id.into(),
                stage_id: stage_id.into(),
            },
        )?;
        let resp = self
            .release_client()
            .get_plan_output(req)
            .await
            .map_err(map_platform_status)?;
        let inner = resp.into_inner();
        Ok(PlanOutput {
            plan_output: inner.plan_output,
            status: inner.status,
            outputs: inner.outputs.into_iter().map(|o| {
                forage_core::platform::PlanDestinationOutput {
                    destination_id: o.destination_id,
                    destination_name: o.destination_name,
                    plan_output: o.plan_output,
                    status: o.status,
                }
            }).collect(),
        })
    }

}

fn convert_policy_evaluation(e: forage_grpc::PolicyEvaluation) -> PolicyEvaluation {
    let policy_type = match e.policy_type {
        1 => "soak_time",
        2 => "branch_restriction",
        3 => "approval",
        _ => "unknown",
    };
    let approval_state = e.external_approval_state.map(|s| convert_approval_state(Some(s)));
    PolicyEvaluation {
        policy_name: e.policy_name,
        policy_type: policy_type.into(),
        passed: e.passed,
        reason: e.reason,
        approval_state,
    }
}

fn convert_approval_state(state: Option<forage_grpc::ExternalApprovalState>) -> ApprovalState {
    match state {
        Some(s) => ApprovalState {
            required_approvals: s.required_approvals,
            current_approvals: s.current_approvals,
            decisions: s
                .decisions
                .into_iter()
                .map(|d| ApprovalDecisionEntry {
                    user_id: d.user_id,
                    username: d.username,
                    decision: d.decision,
                    decided_at: d.decided_at,
                    comment: d.comment,
                })
                .collect(),
        },
        None => ApprovalState {
            required_approvals: 0,
            current_approvals: 0,
            decisions: vec![],
        },
    }
}

#[async_trait::async_trait]
impl ForestRegistry for GrpcForestClient {
    #[tracing::instrument(skip_all)]
    async fn search_components(
        &self,
        access_token: &str,
        query: &str,
        organisation: Option<&str>,
        page: i32,
        page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::SearchComponentsRequest {
                query: query.into(),
                organisation: organisation.unwrap_or_default().into(),
                page: ui_page_to_proto(page),
                page_size,
            },
        )?;
        let resp = self
            .registry_client()
            .search_components(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(ComponentSearchResult {
            total_count: resp.total_count,
            components: resp
                .components
                .into_iter()
                .map(convert_component_summary)
                .collect(),
        })
    }

    #[tracing::instrument(skip_all)]
    async fn get_component_detail(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<ComponentDetail, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetComponentDetailRequest {
                organisation: organisation.into(),
                name: name.into(),
            },
        )?;
        let resp = self
            .registry_client()
            .get_component_detail(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let summary = resp
            .summary
            .map(convert_component_summary)
            .ok_or_else(|| PlatformError::Other("missing component summary".into()))?;
        Ok(ComponentDetail {
            summary,
            versions: resp
                .versions
                .into_iter()
                .map(convert_component_version_info)
                .collect(),
            readme: resp.readme,
            manifest_json: resp.manifest_json,
            owners: resp.owners,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn list_component_versions(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<Vec<ComponentVersionInfo>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListComponentVersionsRequest {
                organisation: organisation.into(),
                name: name.into(),
            },
        )?;
        let resp = self
            .registry_client()
            .list_component_versions(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp
            .versions
            .into_iter()
            .map(convert_component_version_info)
            .collect())
    }

    #[tracing::instrument(skip_all)]
    async fn get_component_manifest(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<String, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::GetComponentManifestRequest {
                organisation: organisation.into(),
                name: name.into(),
                version: version.into(),
            },
        )?;
        let resp = self
            .registry_client()
            .get_component_manifest(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp.manifest_json)
    }

    #[tracing::instrument(skip_all)]
    async fn search_public_components(
        &self,
        query: &str,
        organisation: Option<&str>,
        page: i32,
        page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError> {
        // No bearer header — the server marks this RPC as AuthMode::None
        // and would ignore one anyway. Anything we attach here would
        // amount to handing the service-account key to anonymous traffic.
        let req = Request::new(forage_grpc::SearchPublicComponentsRequest {
            query: query.into(),
            organisation: organisation.unwrap_or_default().into(),
            page: ui_page_to_proto(page),
            page_size,
        });
        let resp = self
            .registry_client()
            .search_public_components(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(ComponentSearchResult {
            total_count: resp.total_count,
            components: resp
                .components
                .into_iter()
                .map(convert_component_summary)
                .collect(),
        })
    }

    #[tracing::instrument(skip_all)]
    async fn get_public_component_detail(
        &self,
        organisation: &str,
        name: &str,
    ) -> Result<ComponentDetail, PlatformError> {
        let req = Request::new(forage_grpc::GetPublicComponentDetailRequest {
            organisation: organisation.into(),
            name: name.into(),
        });
        let resp = self
            .registry_client()
            .get_public_component_detail(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        let summary = resp
            .summary
            .map(convert_component_summary)
            .ok_or_else(|| PlatformError::Other("missing component summary".into()))?;
        Ok(ComponentDetail {
            summary,
            versions: resp
                .versions
                .into_iter()
                .map(convert_component_version_info)
                .collect(),
            readme: resp.readme,
            manifest_json: resp.manifest_json,
            owners: resp.owners,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn get_public_component_manifest(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> Result<String, PlatformError> {
        let req = Request::new(forage_grpc::GetPublicComponentManifestRequest {
            organisation: organisation.into(),
            name: name.into(),
            version: version.into(),
        });
        let resp = self
            .registry_client()
            .get_public_component_manifest(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();
        Ok(resp.manifest_json)
    }

    #[tracing::instrument(skip_all)]
    async fn list_org_tools(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<ToolSummary>, PlatformError> {
        let req = platform_authed_request(
            access_token,
            forage_grpc::ListOrgToolsRequest {
                organisation: organisation.into(),
            },
        )?;
        let mut stream = self
            .registry_client()
            .list_org_tools(req)
            .await
            .map_err(map_platform_status)?
            .into_inner();

        let mut tools = Vec::new();
        loop {
            match stream.message().await {
                Ok(Some(entry)) => tools.push(convert_tool_summary(entry)),
                Ok(None) => break,
                Err(status) => return Err(map_platform_status(status)),
            }
        }
        Ok(tools)
    }
}

fn convert_component_summary(s: forage_grpc::ComponentSummary) -> ComponentSummary {
    let shape = convert_shape(s.shape);
    let tool = s.tool.map(convert_tool_facet);
    let upstream_host = s.upstream_host;
    ComponentSummary {
        organisation: s.organisation,
        name: s.name,
        latest_version: s.latest_version,
        kind: s.kind,
        description: s.description,
        created_at: s.created_at,
        updated_at: s.updated_at,
        version_count: s.version_count,
        contracts: s.contracts,
        visibility: s.visibility,
        shape,
        tool,
        methods: s.methods,
        upstream_host,
    }
}

fn convert_tool_facet(t: forage_grpc::ToolFacet) -> forage_core::registry::ToolFacet {
    forage_core::registry::ToolFacet {
        name: t.name,
        argv_passthrough: t.argv_passthrough,
        description: t.description,
    }
}

/// Map the proto enum (as i32, since prost emits `Enumeration` as i32 fields)
/// to the domain `ToolShape`. Unknown / future variants degrade to
/// `ToolShape::Unknown`, matching the spec's forward-compat rule.
fn convert_shape(raw: i32) -> forage_core::registry::ToolShape {
    use forage_core::registry::ToolShape;
    match forage_grpc::ComponentShape::try_from(raw) {
        Ok(forage_grpc::ComponentShape::Component) => ToolShape::Component,
        Ok(forage_grpc::ComponentShape::Hybrid) => ToolShape::Hybrid,
        Ok(forage_grpc::ComponentShape::ToolBinary) => ToolShape::ToolBinary,
        Ok(forage_grpc::ComponentShape::ToolExternal) => ToolShape::ToolExternal,
        Ok(forage_grpc::ComponentShape::Unspecified) | Err(_) => ToolShape::Unknown,
    }
}

fn convert_tool_summary(e: forage_grpc::OrgToolEntry) -> forage_core::registry::ToolSummary {
    use forage_core::registry::ToolSummary;
    let shape = convert_shape(e.shape);
    let (description, argv_passthrough) = match e.tool {
        Some(t) => (t.description, t.argv_passthrough),
        None => (String::new(), false),
    };
    ToolSummary {
        organisation: e.organisation,
        name: e.name,
        latest_version: e.latest_version,
        shape,
        description,
        argv_passthrough,
        upstream_host: e.upstream_host,
    }
}

fn convert_component_version_info(v: forage_grpc::ComponentVersionInfo) -> ComponentVersionInfo {
    ComponentVersionInfo {
        version: v.version,
        protocol_version: v.protocol_version,
        kind: v.kind,
        platforms: v.platforms,
    }
}

/// Translate a 1-indexed UI page to the 0-indexed proto page.
/// `SearchComponentsRequest.page` is documented as 0-indexed in `forest.v1.rs`,
/// while routes use `params.page.unwrap_or(1).max(1)` (1-indexed). Without this
/// translation, page=1 produced offset=20 server-side and hid the first page.
fn ui_page_to_proto(ui_page: i32) -> i32 {
    ui_page.saturating_sub(1).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_org(id: &str, name: &str) -> forage_grpc::Organisation {
        forage_grpc::Organisation {
            organisation_id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn make_artifact(slug: &str, ctx: Option<forage_grpc::ArtifactContext>) -> forage_grpc::Artifact {
        forage_grpc::Artifact {
            artifact_id: "a1".into(),
            slug: slug.into(),
            context: ctx,
            created_at: "2026-01-01".into(),
            ..Default::default()
        }
    }

    #[test]
    fn convert_organisations_pairs_orgs_with_roles() {
        let orgs = vec![make_org("o1", "alpha"), make_org("o2", "beta")];
        let roles = vec!["owner".into(), "member".into()];

        let result = convert_organisations(orgs, roles);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "alpha");
        assert_eq!(result[0].role, "owner");
        assert_eq!(result[1].name, "beta");
        assert_eq!(result[1].role, "member");
    }

    #[test]
    fn convert_organisations_truncates_when_roles_shorter() {
        let orgs = vec![make_org("o1", "alpha"), make_org("o2", "beta")];
        let roles = vec!["owner".into()]; // only 1 role for 2 orgs

        let result = convert_organisations(orgs, roles);
        // zip truncates to shorter iterator
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "alpha");
    }

    #[test]
    fn convert_organisations_empty() {
        let result = convert_organisations(vec![], vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn convert_artifact_with_full_context() {
        let a = make_artifact("my-api", Some(forage_grpc::ArtifactContext {
            title: "My API".into(),
            description: Some("A cool API".into()),
            ..Default::default()
        }));

        let result = convert_artifact(a);
        assert_eq!(result.slug, "my-api");
        assert_eq!(result.context.title, "My API");
        assert_eq!(result.context.description.as_deref(), Some("A cool API"));
    }

    #[test]
    fn convert_artifact_empty_description_becomes_none() {
        let a = make_artifact("my-api", Some(forage_grpc::ArtifactContext {
            title: "My API".into(),
            description: Some(String::new()),
            ..Default::default()
        }));

        let result = convert_artifact(a);
        assert!(result.context.description.is_none());
    }

    #[test]
    fn convert_artifact_missing_context_uses_defaults() {
        let a = make_artifact("my-api", None);

        let result = convert_artifact(a);
        assert_eq!(result.context.title, "");
        assert!(result.context.description.is_none());
    }

    #[test]
    fn convert_artifact_none_description_stays_none() {
        let a = make_artifact("my-api", Some(forage_grpc::ArtifactContext {
            title: "My API".into(),
            description: None,
            ..Default::default()
        }));

        let result = convert_artifact(a);
        assert!(result.context.description.is_none());
    }

    #[test]
    fn ui_page_to_proto_translates_one_indexed_to_zero_indexed() {
        assert_eq!(ui_page_to_proto(1), 0);
        assert_eq!(ui_page_to_proto(2), 1);
        assert_eq!(ui_page_to_proto(10), 9);
    }

    #[test]
    fn ui_page_to_proto_clamps_zero_and_negative_to_zero() {
        assert_eq!(ui_page_to_proto(0), 0);
        assert_eq!(ui_page_to_proto(-5), 0);
    }

    // ── Tools (TASKS/018 + specs/features/007) ───────────────────────

    fn make_tool_entry(
        shape: forage_grpc::ComponentShape,
        upstream_host: &str,
        tool: Option<forage_grpc::ToolFacet>,
    ) -> forage_grpc::OrgToolEntry {
        forage_grpc::OrgToolEntry {
            organisation: "cuteorg".into(),
            name: "forest-hello".into(),
            latest_version: "0.1.0".into(),
            tool,
            shape: shape as i32,
            upstream_host: upstream_host.into(),
        }
    }

    #[test]
    fn convert_tool_summary_maps_all_fields() {
        // P5 — every field on OrgToolEntry survives the conversion intact.
        let entry = make_tool_entry(
            forage_grpc::ComponentShape::ToolBinary,
            "",
            Some(forage_grpc::ToolFacet {
                name: "forest-hello".into(),
                argv_passthrough: true,
                description: "Print a friendly greeting".into(),
            }),
        );
        let s = convert_tool_summary(entry);
        assert_eq!(s.organisation, "cuteorg");
        assert_eq!(s.name, "forest-hello");
        assert_eq!(s.latest_version, "0.1.0");
        assert_eq!(s.shape, forage_core::registry::ToolShape::ToolBinary);
        assert_eq!(s.description, "Print a friendly greeting");
        assert!(s.argv_passthrough);
        assert_eq!(s.upstream_host, "");
    }

    #[test]
    fn convert_tool_summary_handles_missing_facet() {
        // E3 — a tool with no facet (shouldn't happen post-server-validation,
        // but the conversion stays total). Description falls back to empty,
        // argv_passthrough to false.
        let entry = make_tool_entry(forage_grpc::ComponentShape::ToolExternal, "github.com", None);
        let s = convert_tool_summary(entry);
        assert_eq!(s.description, "");
        assert!(!s.argv_passthrough);
        assert_eq!(s.upstream_host, "github.com");
        assert_eq!(s.shape, forage_core::registry::ToolShape::ToolExternal);
    }

    #[test]
    fn convert_shape_maps_every_proto_variant() {
        use forage_core::registry::ToolShape;
        assert_eq!(
            convert_shape(forage_grpc::ComponentShape::Component as i32),
            ToolShape::Component
        );
        assert_eq!(
            convert_shape(forage_grpc::ComponentShape::Hybrid as i32),
            ToolShape::Hybrid
        );
        assert_eq!(
            convert_shape(forage_grpc::ComponentShape::ToolBinary as i32),
            ToolShape::ToolBinary
        );
        assert_eq!(
            convert_shape(forage_grpc::ComponentShape::ToolExternal as i32),
            ToolShape::ToolExternal
        );
    }

    #[test]
    fn convert_shape_unknown_for_unspecified_and_invalid() {
        // Forward-compat: an unknown variant (e.g. a future server adds one)
        // degrades to Unknown rather than panicking.
        use forage_core::registry::ToolShape;
        assert_eq!(
            convert_shape(forage_grpc::ComponentShape::Unspecified as i32),
            ToolShape::Unknown
        );
        assert_eq!(convert_shape(999), ToolShape::Unknown);
    }

    // ─── map_status: error code translation ──────────────────────────
    //
    // The route layer in `routes/auth.rs::complete_link_flow` pattern-matches
    // on the `AuthError::AlreadyExists(msg)` body to differentiate
    // "already linked to another user" (cross-user conflict, 409) from
    // "already linked to same user" (idempotent re-link). These tests
    // pin the contract between Forest's friendly constraint messages
    // (in `repositories/error.rs`) and the route layer's substring
    // matching.

    #[test]
    fn map_status_translates_cross_user_constraint_message() {
        let status = tonic::Status::already_exists(
            "this external account is already linked to another user",
        );
        match map_status(status) {
            AuthError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("already linked to another user"),
                    "route layer relies on this substring; got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn map_status_translates_same_user_constraint_message() {
        let status = tonic::Status::already_exists(
            "user already has an account linked for this provider",
        );
        match map_status(status) {
            AuthError::AlreadyExists(msg) => {
                // This message is what the route layer falls through to —
                // it triggers `already_linked_<provider>` (vs `already_linked_other_<provider>`).
                assert!(
                    msg.contains("user already has an account linked"),
                    "expected the same-user message verbatim; got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn map_status_unauthenticated_maps_to_invalid_credentials() {
        let status = tonic::Status::unauthenticated("invalid token");
        assert!(matches!(
            map_status(status),
            AuthError::InvalidCredentials
        ));
    }

    #[test]
    fn map_status_permission_denied_preserves_message() {
        let status = tonic::Status::permission_denied(
            "you can only link providers to your own account",
        );
        match map_status(status) {
            AuthError::PermissionDenied(msg) => {
                assert!(msg.contains("link providers to your own account"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn map_status_unavailable_maps_to_unavailable_with_message() {
        let status = tonic::Status::unavailable("forest restarting");
        match map_status(status) {
            AuthError::Unavailable(msg) => assert!(msg.contains("forest restarting")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    // ─── convert_oauth_connection_to_linked: extras handling ─────────

    #[test]
    fn convert_oauth_connection_decodes_provider_data_extras() {
        let conn = forage_grpc::OAuthConnection {
            provider: forage_grpc::OAuthProvider::OauthProviderGithub as i32,
            provider_user_id: "12345".into(),
            provider_email: "kasper@understory.io".into(),
            linked_at: None,
            provider_display_name: "kjuulh".into(),
            provider_data_json: r#"{"login":"kjuulh","avatar_url":"https://example.com/a.png","name":"Kasper Hermansen"}"#.into(),
        };
        let id = convert_oauth_connection_to_linked(conn).expect("github should convert");
        assert_eq!(id.provider, forage_core::auth::LinkedProvider::GitHub);
        assert_eq!(id.external_id, "12345");
        // provider_display_name wins over the JSON-embedded login.
        assert_eq!(id.display_name, "kjuulh");
        assert_eq!(id.email.as_deref(), Some("kasper@understory.io"));
        assert_eq!(id.avatar_url.as_deref(), Some("https://example.com/a.png"));
    }

    #[test]
    fn convert_oauth_connection_falls_back_when_extras_empty() {
        let conn = forage_grpc::OAuthConnection {
            provider: forage_grpc::OAuthProvider::OauthProviderGoogle as i32,
            provider_user_id: "g-sub".into(),
            provider_email: "kasper@understory.io".into(),
            linked_at: None,
            provider_display_name: String::new(),
            provider_data_json: String::new(),
        };
        let id = convert_oauth_connection_to_linked(conn).expect("google should convert");
        // With no extras and no display_name, falls back to email.
        assert_eq!(id.display_name, "kasper@understory.io");
        assert_eq!(id.avatar_url, None);
    }

    #[test]
    fn convert_oauth_connection_skips_unknown_providers() {
        let conn = forage_grpc::OAuthConnection {
            provider: forage_grpc::OAuthProvider::OauthProviderMagicLink as i32,
            provider_user_id: "x".into(),
            provider_email: String::new(),
            linked_at: None,
            provider_display_name: String::new(),
            provider_data_json: String::new(),
        };
        assert!(
            convert_oauth_connection_to_linked(conn).is_none(),
            "magic-link is not surfaced on the linked-accounts UI"
        );
    }

    #[test]
    fn convert_oauth_connection_tolerates_malformed_provider_data_json() {
        // Defensive: a corrupt JSON blob shouldn't drop the whole identity.
        let conn = forage_grpc::OAuthConnection {
            provider: forage_grpc::OAuthProvider::OauthProviderGithub as i32,
            provider_user_id: "12345".into(),
            provider_email: "kasper@understory.io".into(),
            linked_at: None,
            provider_display_name: "kjuulh".into(),
            provider_data_json: "not-json{".into(),
        };
        let id = convert_oauth_connection_to_linked(conn).expect("identity should still render");
        // display_name still wins because the field is parsed separately.
        assert_eq!(id.display_name, "kjuulh");
    }
}
