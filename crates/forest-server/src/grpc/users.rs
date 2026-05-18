use chrono::{Days, Utc};
use forest_grpc_interface::{users_service_server::UsersService, *};
use sha2::Digest;
use totp_rs::{Algorithm, Secret, TOTP};
use uuid::Uuid;

use super::error;
use crate::{
    actor::Actor,
    services::{registration_policy::RegistrationPolicyState, users::UserServiceState},
    state::State,
    tokens::TokenServiceState,
};

pub struct UsersServer {
    pub state: State,
}

impl UsersServer {
    fn service(&self) -> crate::services::users::UserService {
        self.state.user_service()
    }

    /// Apply the registration email-domain regex (if configured). Maps the
    /// pure-core `DomainNotAllowed` error to a generic `permission_denied`
    /// gRPC status that does *not* leak the configured pattern to clients.
    /// On rejection, logs the domain (text after the last `@`) only —
    /// never the full address — so operators can tune the regex without
    /// pulling PII into logs.
    fn enforce_registration_policy(&self, email: &str) -> Result<(), tonic::Status> {
        if self.state.registration_policy().check_email(email).is_err() {
            let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("<no-@>");
            tracing::warn!(domain = %domain, "registration rejected by domain policy");
            return Err(tonic::Status::permission_denied(
                "registration is restricted to allowed email domains",
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl UsersService for UsersServer {
    // ── Authentication ───────────────────────────────────────────────

    async fn register(
        &self,
        request: tonic::Request<RegisterRequest>,
    ) -> std::result::Result<tonic::Response<RegisterResponse>, tonic::Status> {
        let req = request.into_inner();

        self.enforce_registration_policy(&req.email)?;

        let registered = self
            .service()
            .register(&req.username, &req.email, &req.password)
            .await
            .map_err(error::to_status)?;

        let profile = self
            .service()
            .get_user(registered.user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::internal("user not found after registration"))?;

        // When verification is required, the registered user has no
        // verified email yet (native register inserts verified=false), so
        // we return user info but no tokens. The caller (forage) is
        // expected to drive the verification flow.
        if self.state.config.require_email_verification {
            return Ok(tonic::Response::new(RegisterResponse {
                user: Some(profile_to_grpc_user(profile)),
                tokens: None,
                email_verification_required: true,
            }));
        }

        let (refresh_token, hash) = self
            .state
            .tokens()
            .generate_refresh_token()
            .map_err(error::to_status)?;

        let expires = Utc::now()
            .checked_add_days(Days::new(30))
            .expect("to be able to add 30 days");

        let session = self
            .state
            .user_service()
            .create_session(profile.user_id, &hash, Some(expires))
            .await
            .map_err(error::to_status)?;

        let access_token = self
            .state
            .tokens()
            .issue_access_token(
                &profile.user_id.to_string(),
                &session.session_id.to_string(),
                vec![],
            )
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(RegisterResponse {
            user: Some(profile_to_grpc_user(profile)),
            tokens: Some(AuthTokens {
                access_token: access_token.as_string(),
                refresh_token,
                expires_in_seconds: expires.timestamp(),
            }),
            email_verification_required: false,
        }))
    }

    async fn login(
        &self,
        request: tonic::Request<LoginRequest>,
    ) -> std::result::Result<tonic::Response<LoginResponse>, tonic::Status> {
        let req = request.into_inner();

        let authenticated = match req.identifier {
            Some(login_request::Identifier::Username(username)) => {
                self.service()
                    .login_by_username(&username, &req.password)
                    .await
            }
            Some(login_request::Identifier::Email(email)) => {
                self.service().login_by_email(&email, &req.password).await
            }
            None => return Err(tonic::Status::invalid_argument("identifier is required")),
        }
        .map_err(error::to_status)?
        .ok_or_else(|| tonic::Status::unauthenticated("invalid credentials"))?;

        let profile = self
            .service()
            .get_user(authenticated.user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::internal("user not found"))?;

        // Block login when the operator requires email verification and
        // none of the user's emails are verified. Forage uses the
        // canonical detail string "email_not_verified" to render a
        // resend-verification page.
        if self.state.config.require_email_verification {
            let has_verified = self
                .service()
                .user_has_verified_email(profile.user_id)
                .await
                .map_err(error::to_status)?;
            if !has_verified {
                return Err(tonic::Status::failed_precondition("email_not_verified"));
            }
        }

        // Check MFA status — if the user has verified MFA, return a challenge
        // instead of issuing tokens immediately.
        let mfa = self
            .service()
            .get_mfa_for_user(authenticated.user_id)
            .await
            .map_err(error::to_status)?;
        if let Some(mfa) = mfa {
            if mfa.verified {
                let state_data = serde_json::json!({
                    "user_id": authenticated.user_id.to_string(),
                    "type": "mfa_login"
                });
                let state_token = format!("mfa-{}", Uuid::now_v7());
                let expires_mfa =
                    Utc::now() + chrono::Duration::minutes(5);
                self.service()
                    .create_oauth_state(
                        "mfa",
                        &state_token,
                        None,
                        &state_data,
                        Some(expires_mfa),
                    )
                    .await
                    .map_err(error::to_status)?;

                return Ok(tonic::Response::new(LoginResponse {
                    user: None,
                    tokens: None,
                    mfa_required: true,
                    mfa_session_token: state_token,
                }));
            }
        }

        let (refresh_token, hash) = self
            .state
            .tokens()
            .generate_refresh_token()
            .map_err(error::to_status)?;

        let expires = Utc::now()
            .checked_add_days(Days::new(30))
            .expect("to be able to add 30 days");

        let session = self
            .state
            .user_service()
            .create_session(profile.user_id, &hash, Some(expires))
            .await
            .map_err(error::to_status)?;

        let access_token = self
            .state
            .tokens()
            .issue_access_token(
                &profile.user_id.to_string(),
                &session.session_id.to_string(),
                vec![],
            )
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(LoginResponse {
            user: Some(profile_to_grpc_user(profile)),
            tokens: Some(AuthTokens {
                access_token: access_token.as_string(),
                refresh_token,
                expires_in_seconds: expires.timestamp(),
            }),
            mfa_required: false,
            mfa_session_token: String::new(),
        }))
    }

    async fn refresh_token(
        &self,
        request: tonic::Request<RefreshTokenRequest>,
    ) -> std::result::Result<tonic::Response<RefreshTokenResponse>, tonic::Status> {
        let req = request.into_inner();

        let token_hash = self
            .state
            .tokens()
            .get_token_hash(&req.refresh_token)
            .map_err(|e| tonic::Status::unauthenticated(e.to_string()))?;

        let session = self
            .service()
            .validate_session_full(&token_hash)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::unauthenticated("session expired or revoked"))?;

        // Revoke old session
        self.service()
            .logout(session.session_id)
            .await
            .map_err(error::to_status)?;

        // Issue new tokens
        let (refresh_token, hash) = self
            .state
            .tokens()
            .generate_refresh_token()
            .map_err(error::to_status)?;

        let expires = Utc::now()
            .checked_add_days(Days::new(30))
            .expect("to be able to add 30 days");

        let new_session = self
            .service()
            .create_session(session.user_id, &hash, Some(expires))
            .await
            .map_err(error::to_status)?;

        let access_token = self
            .state
            .tokens()
            .issue_access_token(
                &session.user_id.to_string(),
                &new_session.session_id.to_string(),
                vec![],
            )
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(RefreshTokenResponse {
            tokens: Some(AuthTokens {
                access_token: access_token.as_string(),
                refresh_token,
                expires_in_seconds: expires.timestamp(),
            }),
        }))
    }

    async fn logout(
        &self,
        _request: tonic::Request<LogoutRequest>,
    ) -> std::result::Result<tonic::Response<LogoutResponse>, tonic::Status> {
        // TODO: extract session from auth context and revoke
        Err(tonic::Status::unimplemented("not yet implemented"))
    }

    async fn token_info(
        &self,
        request: tonic::Request<TokenInfoRequest>,
    ) -> std::result::Result<tonic::Response<TokenInfoResponse>, tonic::Status> {
        // The auth layer already verified the token and inserted AppClaims.
        // We just read them back — no database hit needed.
        let claims = request
            .extensions()
            .get::<crate::tokens::AppClaims>()
            .ok_or_else(|| tonic::Status::internal("missing claims in request extensions"))?;

        Ok(tonic::Response::new(TokenInfoResponse {
            user_id: claims.user_id.clone(),
            // The JWT exp claim is validated by the auth layer; returning 0 here
            // since the client only cares whether the call succeeds or not.
            expires_at: 0,
        }))
    }

    // ── User CRUD ────────────────────────────────────────────────────

    async fn get_user(
        &self,
        request: tonic::Request<GetUserRequest>,
    ) -> std::result::Result<tonic::Response<GetUserResponse>, tonic::Status> {
        let req = request.into_inner();

        let profile = match req.identifier {
            Some(get_user_request::Identifier::UserId(id)) => {
                let user_id = id
                    .parse::<Uuid>()
                    .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;
                self.service().get_user(user_id).await
            }
            Some(get_user_request::Identifier::Username(username)) => {
                self.service().get_user_by_username(&username).await
            }
            Some(get_user_request::Identifier::Email(email)) => {
                self.service().get_user_by_email(&email).await
            }
            None => return Err(tonic::Status::invalid_argument("identifier is required")),
        }
        .map_err(error::to_status)?
        .ok_or_else(|| tonic::Status::not_found("user not found"))?;

        Ok(tonic::Response::new(GetUserResponse {
            user: Some(profile_to_grpc_user(profile)),
        }))
    }

    async fn update_user(
        &self,
        request: tonic::Request<UpdateUserRequest>,
    ) -> std::result::Result<tonic::Response<UpdateUserResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        if let Some(username) = req.username {
            self.service()
                .update_username(user_id, &username)
                .await
                .map_err(error::to_status)?;
        }

        if let Some(ref url) = req.profile_picture_url {
            let url = if url.is_empty() { None } else { Some(url.as_str()) };
            self.service()
                .update_profile_picture_url(user_id, url)
                .await
                .map_err(error::to_status)?;
        }

        let profile = self
            .service()
            .get_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("user not found"))?;

        Ok(tonic::Response::new(UpdateUserResponse {
            user: Some(profile_to_grpc_user(profile)),
        }))
    }

    async fn delete_user(
        &self,
        request: tonic::Request<DeleteUserRequest>,
    ) -> std::result::Result<tonic::Response<DeleteUserResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        self.service()
            .delete_user(user_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(DeleteUserResponse {}))
    }

    async fn list_users(
        &self,
        request: tonic::Request<ListUsersRequest>,
    ) -> std::result::Result<tonic::Response<ListUsersResponse>, tonic::Status> {
        let req = request.into_inner();
        let page_size = if req.page_size > 0 {
            req.page_size as i64
        } else {
            50
        };
        let offset = req.page_token.parse::<i64>().unwrap_or(0);

        let user_list = self
            .service()
            .list_users(page_size, offset, req.search.as_deref())
            .await
            .map_err(error::to_status)?;

        let next_page_token = if user_list.has_more {
            (offset + page_size).to_string()
        } else {
            String::new()
        };

        Ok(tonic::Response::new(ListUsersResponse {
            users: user_list
                .users
                .into_iter()
                .map(|u| User {
                    user_id: u.user_id.to_string(),
                    username: u.username,
                    created_at: Some(datetime_to_timestamp(u.created_at)),
                    ..Default::default()
                })
                .collect(),
            next_page_token,
            total_count: 0,
        }))
    }

    // ── Stats ────────────────────────────────────────────────────────

    async fn get_user_stats(
        &self,
        request: tonic::Request<GetUserStatsRequest>,
    ) -> std::result::Result<tonic::Response<GetUserStatsResponse>, tonic::Status> {
        let req = request.into_inner();

        let user_id = match req.identifier {
            Some(get_user_stats_request::Identifier::UserId(id)) => id
                .parse::<Uuid>()
                .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?,
            Some(get_user_stats_request::Identifier::Username(username)) => {
                let profile = self
                    .service()
                    .get_user_by_username(&username)
                    .await
                    .map_err(error::to_status)?
                    .ok_or_else(|| tonic::Status::not_found("user not found"))?;
                profile.user_id
            }
            None => return Err(tonic::Status::invalid_argument("identifier is required")),
        };

        let stats = self
            .service()
            .get_user_stats(user_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(GetUserStatsResponse {
            stats: Some(UserStats {
                total_releases: stats.total_releases,
                successful_releases: stats.successful_releases,
                failed_releases: stats.failed_releases,
                in_progress_releases: stats.in_progress_releases,
                total_annotations: stats.total_annotations,
                total_uploads: stats.total_uploads,
            }),
        }))
    }

    // ── Password management ──────────────────────────────────────────

    async fn change_password(
        &self,
        request: tonic::Request<ChangePasswordRequest>,
    ) -> std::result::Result<tonic::Response<ChangePasswordResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        self.service()
            .change_password(user_id, &req.current_password, &req.new_password)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(ChangePasswordResponse {}))
    }

    // ── Email management ─────────────────────────────────────────────

    async fn add_email(
        &self,
        request: tonic::Request<AddEmailRequest>,
    ) -> std::result::Result<tonic::Response<AddEmailResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        self.enforce_registration_policy(&req.email)?;

        self.service()
            .add_email(user_id, &req.email)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(AddEmailResponse {
            email: Some(UserEmail {
                email: req.email,
                verified: false,
            }),
            email_verification_required: self.state.config.require_email_verification,
        }))
    }

    async fn verify_email(
        &self,
        request: tonic::Request<VerifyEmailRequest>,
    ) -> std::result::Result<tonic::Response<VerifyEmailResponse>, tonic::Status> {
        // User-self only: a logged-in user can verify their own email.
        // Cross-user verification by JWT is rejected. The service-account
        // path lives on `confirm_email_verification`.
        let claims = request
            .extensions()
            .get::<crate::tokens::AppClaims>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth"))?;
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;
        if claims.user_id != user_id.to_string() {
            return Err(tonic::Status::permission_denied(
                "verify_email is restricted to the authenticated user",
            ));
        }

        self.service()
            .verify_email(user_id, &req.email)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(VerifyEmailResponse {}))
    }

    async fn confirm_email_verification(
        &self,
        request: tonic::Request<ConfirmEmailVerificationRequest>,
    ) -> std::result::Result<tonic::Response<ConfirmEmailVerificationResponse>, tonic::Status> {
        // Service-account-only. The caller (forage) has already validated
        // a redemption token; we just flip the bit.
        let actor = request.extensions().get::<Actor>().cloned();
        match &actor {
            Some(Actor::ServiceAccount { .. }) => {}
            _ => {
                return Err(tonic::Status::permission_denied(
                    "ConfirmEmailVerification requires service account authentication",
                ));
            }
        }

        let req = request.into_inner();

        // Look up the owning user from the email. The caller is not
        // expected to know user_id.
        let profile = self
            .service()
            .get_user_by_email(&req.email)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("no user owns this email"))?;

        self.service()
            .verify_email(profile.user_id, &req.email)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(ConfirmEmailVerificationResponse {}))
    }

    async fn remove_email(
        &self,
        request: tonic::Request<RemoveEmailRequest>,
    ) -> std::result::Result<tonic::Response<RemoveEmailResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        self.service()
            .remove_email(user_id, &req.email)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(RemoveEmailResponse {}))
    }

    // ── OAuth ────────────────────────────────────────────────────────

    async fn o_auth_login(
        &self,
        request: tonic::Request<OAuthLoginRequest>,
    ) -> std::result::Result<tonic::Response<OAuthLoginResponse>, tonic::Status> {
        // Require service-account auth — only trusted callers (e.g. Forage)
        // can submit pre-verified identity info.
        let actor = request.extensions().get::<Actor>().cloned();
        match &actor {
            Some(Actor::ServiceAccount { .. }) => {}
            _ => {
                return Err(tonic::Status::permission_denied(
                    "OAuth login requires service account authentication",
                ));
            }
        }

        let req = request.into_inner();

        let provider = forest_grpc_interface::OAuthProvider::try_from(req.provider)
            .map_err(|_| tonic::Status::invalid_argument("invalid provider"))?;
        let provider_str = provider.as_str_name().to_lowercase();

        if req.provider_user_id.is_empty() {
            return Err(tonic::Status::invalid_argument("provider_user_id is required"));
        }
        if req.provider_email.is_empty() {
            return Err(tonic::Status::invalid_argument("provider_email is required"));
        }

        // Look up existing user by OAuth identity.
        let existing_user_id = self
            .service()
            .find_user_by_oauth(&provider_str, &req.provider_user_id)
            .await
            .map_err(error::to_status)?;

        let provider_data = if req.provider_data_json.is_empty() {
            None
        } else {
            serde_json::from_str::<serde_json::Value>(&req.provider_data_json).ok()
        };

        let (user_id, is_new_user) = if let Some(uid) = existing_user_id {
            // Known identity — just log them in.
            (uid, false)
        } else if let Some(existing_profile) = self
            .service()
            .get_user_by_email(&req.provider_email)
            .await
            .map_err(error::to_status)?
        {
            // Email already belongs to an existing user (e.g. registered with
            // password or another OAuth provider). Link this new provider and
            // log them in.
            self.service()
                .link_oauth_provider(
                    existing_profile.user_id,
                    &provider_str,
                    &req.provider_user_id,
                    Some(&req.provider_email),
                    provider_data.as_ref(),
                )
                .await
                .map_err(error::to_status)?;
            (existing_profile.user_id, false)
        } else {
            // Completely new user — create account with placeholder username.
            // Apply the registration domain regex here (signup branch only;
            // login branches above are exempt so existing users aren't
            // locked out when the operator tightens the regex).
            self.enforce_registration_policy(&req.provider_email)?;

            let placeholder_username = format!("user-{}", Uuid::now_v7().simple());
            let registered = self
                .service()
                .register_oauth_user(&placeholder_username, &req.provider_email)
                .await
                .map_err(error::to_status)?;

            self.service()
                .link_oauth_provider(
                    registered.user_id,
                    &provider_str,
                    &req.provider_user_id,
                    Some(&req.provider_email),
                    provider_data.as_ref(),
                )
                .await
                .map_err(error::to_status)?;

            (registered.user_id, true)
        };

        // Set profile picture from provider data if user doesn't already have one.
        // Uses a conditional update to avoid race conditions with concurrent logins.
        if let Some(ref data) = provider_data {
            if let Some(picture_url) = data.get("picture_url").and_then(|v| v.as_str()) {
                if !picture_url.is_empty() && picture_url.starts_with("https://") {
                    self.service()
                        .set_profile_picture_url_if_unset(user_id, picture_url)
                        .await
                        .map_err(error::to_status)?;
                }
            }
        }

        // Load user profile.
        let profile = self
            .service()
            .get_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::internal("user not found after OAuth login"))?;

        // Issue tokens (same pattern as register/login).
        let (refresh_token, hash) = self
            .state
            .tokens()
            .generate_refresh_token()
            .map_err(error::to_status)?;

        let expires = Utc::now()
            .checked_add_days(Days::new(30))
            .expect("to be able to add 30 days");

        let session = self
            .state
            .user_service()
            .create_session(user_id, &hash, Some(expires))
            .await
            .map_err(error::to_status)?;

        let access_token = self
            .state
            .tokens()
            .issue_access_token(
                &user_id.to_string(),
                &session.session_id.to_string(),
                vec![],
            )
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(OAuthLoginResponse {
            user: Some(profile_to_grpc_user(profile)),
            tokens: Some(AuthTokens {
                access_token: access_token.as_string(),
                refresh_token,
                expires_in_seconds: expires.timestamp(),
            }),
            is_new_user,
        }))
    }

    async fn link_o_auth_provider(
        &self,
        request: tonic::Request<LinkOAuthProviderRequest>,
    ) -> std::result::Result<tonic::Response<LinkOAuthProviderResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        let provider = forest_grpc_interface::OAuthProvider::try_from(req.provider)
            .map_err(|_| tonic::Status::invalid_argument("invalid provider"))?;

        let provider_str = provider.as_str_name().to_lowercase();
        let provider_email = if req.provider_email.is_empty() {
            None
        } else {
            Some(req.provider_email.as_str())
        };

        self.service()
            .link_oauth_provider(user_id, &provider_str, &req.provider_user_id, provider_email, None)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(LinkOAuthProviderResponse {
            connection: None,
        }))
    }

    async fn unlink_o_auth_provider(
        &self,
        request: tonic::Request<UnlinkOAuthProviderRequest>,
    ) -> std::result::Result<tonic::Response<UnlinkOAuthProviderResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        let provider = forest_grpc_interface::OAuthProvider::try_from(req.provider)
            .map_err(|_| tonic::Status::invalid_argument("invalid provider"))?;

        self.service()
            .unlink_oauth_provider(user_id, &provider.as_str_name().to_lowercase())
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(UnlinkOAuthProviderResponse {}))
    }

    // ── Personal access tokens ───────────────────────────────────────

    async fn create_personal_access_token(
        &self,
        request: tonic::Request<CreatePersonalAccessTokenRequest>,
    ) -> std::result::Result<tonic::Response<CreatePersonalAccessTokenResponse>, tonic::Status>
    {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        let mut raw_bytes = [0u8; 32];
        rand::fill(&mut raw_bytes[..]);
        let raw_token = hex::encode(raw_bytes);
        let token_hash = sha2::Sha256::digest(raw_token.as_bytes()).to_vec();
        let scopes = serde_json::to_value(&req.scopes)
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let expires_at = if req.expires_in_seconds > 0 {
            Some(chrono::Utc::now() + chrono::Duration::seconds(req.expires_in_seconds))
        } else {
            None
        };

        let token_id = self
            .service()
            .create_personal_access_token(user_id, &req.name, &token_hash, &scopes, expires_at)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(CreatePersonalAccessTokenResponse {
            token: Some(PersonalAccessToken {
                token_id: token_id.to_string(),
                name: req.name,
                scopes: req.scopes,
                expires_at: expires_at.map(datetime_to_timestamp),
                last_used: None,
                created_at: Some(datetime_to_timestamp(chrono::Utc::now())),
            }),
            raw_token,
        }))
    }

    async fn list_personal_access_tokens(
        &self,
        request: tonic::Request<ListPersonalAccessTokensRequest>,
    ) -> std::result::Result<tonic::Response<ListPersonalAccessTokensResponse>, tonic::Status> {
        let req = request.into_inner();
        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        let tokens = self
            .service()
            .list_personal_access_tokens(user_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(ListPersonalAccessTokensResponse {
            tokens: tokens.into_iter().map(pat_info_to_grpc).collect(),
        }))
    }

    async fn delete_personal_access_token(
        &self,
        request: tonic::Request<DeletePersonalAccessTokenRequest>,
    ) -> std::result::Result<tonic::Response<DeletePersonalAccessTokenResponse>, tonic::Status>
    {
        let req = request.into_inner();
        let token_id = req
            .token_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid token_id"))?;

        self.service()
            .delete_personal_access_token(token_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(DeletePersonalAccessTokenResponse {}))
    }

    // ── MFA ──────────────────────────────────────────────────────────

    async fn setup_mfa(
        &self,
        request: tonic::Request<SetupMfaRequest>,
    ) -> std::result::Result<tonic::Response<SetupMfaResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<crate::tokens::AppClaims>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth"))?;
        let req = request.into_inner();

        let mfa_type = MfaType::try_from(req.mfa_type)
            .map_err(|_| tonic::Status::invalid_argument("invalid mfa_type"))?;
        if mfa_type == MfaType::Unspecified {
            return Err(tonic::Status::invalid_argument("mfa_type is required"));
        }

        let user_id: Uuid = claims
            .user_id
            .parse()
            .map_err(|_| tonic::Status::internal("invalid user_id in claims"))?;

        // Get user email for the provisioning URI.
        let profile = self
            .service()
            .get_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("user not found"))?;
        let email = profile
            .emails
            .first()
            .map(|e| e.email.as_str())
            .unwrap_or("user");

        // Generate a cryptographically random TOTP secret.
        let secret = Secret::generate_secret();
        let secret_bytes = secret
            .to_bytes()
            .map_err(|e| tonic::Status::internal(format!("secret encoding error: {e}")))?;
        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret_bytes.clone(),
            Some("Forest".to_string()),
            email.to_string(),
        )
        .map_err(|e| tonic::Status::internal(format!("TOTP setup error: {e}")))?;
        let provisioning_uri = totp.get_url();
        let secret_base32 = secret.to_encoded().to_string();

        let mfa_id = self
            .service()
            .setup_mfa(user_id, "totp", &secret_bytes)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(SetupMfaResponse {
            mfa_id: mfa_id.to_string(),
            provisioning_uri,
            secret: secret_base32,
        }))
    }

    async fn verify_mfa(
        &self,
        request: tonic::Request<VerifyMfaRequest>,
    ) -> std::result::Result<tonic::Response<VerifyMfaResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<crate::tokens::AppClaims>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth"))?;
        let req = request.into_inner();

        let user_id: Uuid = claims
            .user_id
            .parse()
            .map_err(|_| tonic::Status::internal("invalid user_id in claims"))?;
        let mfa_id: Uuid = req
            .mfa_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid mfa_id"))?;

        // Load MFA record and verify it belongs to the calling user.
        let mfa = self
            .service()
            .get_mfa_for_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("MFA setup not found"))?;

        if mfa.id != mfa_id {
            return Err(tonic::Status::permission_denied("MFA ID mismatch"));
        }

        // Validate the TOTP code.
        let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, mfa.secret.clone(), None, String::new())
            .map_err(|e| tonic::Status::internal(format!("TOTP init error: {e}")))?;
        if !totp.check_current(&req.code).unwrap_or(false) {
            return Err(tonic::Status::unauthenticated("invalid TOTP code"));
        }

        self.service()
            .verify_mfa(mfa_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(VerifyMfaResponse {}))
    }

    async fn disable_mfa(
        &self,
        request: tonic::Request<DisableMfaRequest>,
    ) -> std::result::Result<tonic::Response<DisableMfaResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<crate::tokens::AppClaims>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth"))?;
        let req = request.into_inner();

        let user_id: Uuid = claims
            .user_id
            .parse()
            .map_err(|_| tonic::Status::internal("invalid user_id in claims"))?;

        let mfa = self
            .service()
            .get_mfa_for_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("MFA not enabled"))?;

        // Require the current TOTP code to confirm the disable request.
        let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, mfa.secret.clone(), None, String::new())
            .map_err(|e| tonic::Status::internal(format!("TOTP init error: {e}")))?;
        if !totp.check_current(&req.code).unwrap_or(false) {
            return Err(tonic::Status::unauthenticated("invalid TOTP code"));
        }

        self.service()
            .disable_mfa(user_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(DisableMfaResponse {}))
    }

    async fn verify_login_mfa(
        &self,
        request: tonic::Request<VerifyLoginMfaRequest>,
    ) -> std::result::Result<tonic::Response<VerifyLoginMfaResponse>, tonic::Status> {
        let req = request.into_inner();

        // Consume the MFA session token — single-use, expires in 5 minutes.
        let state_info = self
            .service()
            .consume_oauth_state(&req.mfa_session_token)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::unauthenticated("invalid or expired MFA session"))?;

        if state_info.data.get("type").and_then(|v| v.as_str()) != Some("mfa_login") {
            return Err(tonic::Status::unauthenticated("invalid MFA session type"));
        }

        let user_id: Uuid = state_info
            .data
            .get("user_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| tonic::Status::internal("invalid MFA session data"))?;

        // Load MFA record and validate the TOTP code.
        let mfa = self
            .service()
            .get_mfa_for_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::internal("MFA not found for user"))?;

        let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, mfa.secret.clone(), None, String::new())
            .map_err(|e| tonic::Status::internal(format!("TOTP init error: {e}")))?;
        if !totp.check_current(&req.code).unwrap_or(false) {
            return Err(tonic::Status::unauthenticated("invalid TOTP code"));
        }

        // Record successful use.
        self.service()
            .touch_mfa(mfa.id)
            .await
            .map_err(error::to_status)?;

        // Load the full user profile for the response.
        let profile = self
            .service()
            .get_user(user_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::internal("user not found"))?;

        // Issue tokens (same pattern as login/register).
        let (refresh_token, hash) = self
            .state
            .tokens()
            .generate_refresh_token()
            .map_err(error::to_status)?;

        let expires = Utc::now()
            .checked_add_days(Days::new(30))
            .expect("to be able to add 30 days");

        let session = self
            .state
            .user_service()
            .create_session(user_id, &hash, Some(expires))
            .await
            .map_err(error::to_status)?;

        let access_token = self
            .state
            .tokens()
            .issue_access_token(
                &user_id.to_string(),
                &session.session_id.to_string(),
                vec![],
            )
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(VerifyLoginMfaResponse {
            user: Some(profile_to_grpc_user(profile)),
            tokens: Some(AuthTokens {
                access_token: access_token.as_string(),
                refresh_token,
                expires_in_seconds: expires.timestamp(),
            }),
        }))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn profile_to_grpc_user(profile: crate::services::users::UserProfile) -> User {
    User {
        user_id: profile.user_id.to_string(),
        username: profile.username,
        emails: profile
            .emails
            .into_iter()
            .map(|e| UserEmail {
                email: e.email,
                verified: e.verified,
            })
            .collect(),
        oauth_connections: profile
            .oauth_connections
            .into_iter()
            .map(|c| OAuthConnection {
                provider: provider_str_to_enum(&c.provider) as i32,
                provider_user_id: c.provider_user_id,
                provider_email: c.provider_email.unwrap_or_default(),
                linked_at: Some(datetime_to_timestamp(c.linked_at)),
            })
            .collect(),
        mfa_enabled: profile.mfa_enabled,
        created_at: Some(datetime_to_timestamp(profile.created_at)),
        updated_at: Some(datetime_to_timestamp(profile.updated_at)),
        profile_picture_url: profile.profile_picture_url,
    }
}

fn pat_info_to_grpc(info: crate::services::users::PersonalAccessTokenInfo) -> PersonalAccessToken {
    let scopes: Vec<String> = serde_json::from_value(info.scopes).unwrap_or_default();
    PersonalAccessToken {
        token_id: info.id.to_string(),
        name: info.name,
        scopes,
        expires_at: info.expires_at.map(datetime_to_timestamp),
        last_used: info.last_used.map(datetime_to_timestamp),
        created_at: Some(datetime_to_timestamp(info.created_at)),
    }
}

fn provider_str_to_enum(provider: &str) -> forest_grpc_interface::OAuthProvider {
    match provider {
        "github" | "oauth_provider_github" => {
            forest_grpc_interface::OAuthProvider::OauthProviderGithub
        }
        "google" | "oauth_provider_google" => {
            forest_grpc_interface::OAuthProvider::OauthProviderGoogle
        }
        "gitlab" | "oauth_provider_gitlab" => {
            forest_grpc_interface::OAuthProvider::OauthProviderGitlab
        }
        "microsoft" | "oauth_provider_microsoft" => {
            forest_grpc_interface::OAuthProvider::OauthProviderMicrosoft
        }
        _ => forest_grpc_interface::OAuthProvider::OauthProviderUnspecified,
    }
}

fn datetime_to_timestamp(dt: chrono::DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}
