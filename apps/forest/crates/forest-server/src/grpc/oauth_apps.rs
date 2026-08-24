use forest_grpc_interface::{o_auth_apps_service_server::OAuthAppsService, *};
use uuid::Uuid;

use super::error;
use crate::{
    actor::Actor,
    grpc::authorize::{self, OrgRole},
    services::oauth_apps::{OAuthApp, OAuthAppServiceState},
    state::State,
};

pub struct OAuthAppsServer {
    pub state: State,
}

impl OAuthAppsServer {
    fn service(&self) -> crate::services::oauth_apps::OAuthAppService {
        self.state.oauth_app_service()
    }

    /// The OIDC issuer — the public web-app URL (where discovery is served).
    fn issuer(&self) -> &str {
        self.state.config.web_app_url.as_deref().unwrap_or_default()
    }

    /// Authorize the caller as an admin of `organisation_id` and return the
    /// resolved org id. All OAuth-app management is admin-only.
    async fn authorize_admin(
        &self,
        request_actor: &Actor,
        organisation_id: &str,
    ) -> Result<Uuid, tonic::Status> {
        let org_id = parse_uuid(organisation_id, "organisation_id")?;
        authorize::require_org_access_by_id(&self.state.db, request_actor, org_id, OrgRole::Admin)
            .await?;
        Ok(org_id)
    }
}

#[async_trait::async_trait]
impl OAuthAppsService for OAuthAppsServer {
    async fn create_o_auth_app(
        &self,
        request: tonic::Request<CreateOAuthAppRequest>,
    ) -> Result<tonic::Response<CreateOAuthAppResponse>, tonic::Status> {
        let actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .into_actor();
        let req = request.into_inner();
        let org_id = self.authorize_admin(&actor, &req.organisation_id).await?;
        let created_by = acting_user_id(&actor)?;

        let created = self
            .service()
            .create_app(
                org_id,
                created_by,
                &req.name,
                &req.description,
                &req.homepage_url,
                &req.redirect_uris,
                &req.scopes,
                &req.grant_types,
            )
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(CreateOAuthAppResponse {
            app: Some(to_proto(created.app)),
            client_secret: created.client_secret,
        }))
    }

    async fn list_o_auth_apps(
        &self,
        request: tonic::Request<ListOAuthAppsRequest>,
    ) -> Result<tonic::Response<ListOAuthAppsResponse>, tonic::Status> {
        let actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .into_actor();
        let req = request.into_inner();
        let org_id = self.authorize_admin(&actor, &req.organisation_id).await?;

        let apps = self
            .service()
            .list_apps(org_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(ListOAuthAppsResponse {
            apps: apps.into_iter().map(to_proto).collect(),
        }))
    }

    async fn get_o_auth_app(
        &self,
        request: tonic::Request<GetOAuthAppRequest>,
    ) -> Result<tonic::Response<GetOAuthAppResponse>, tonic::Status> {
        let actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .into_actor();
        let req = request.into_inner();
        let org_id = self.authorize_admin(&actor, &req.organisation_id).await?;
        let app_id = parse_uuid(&req.app_id, "app_id")?;

        let app = self
            .service()
            .get_app(org_id, app_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("oauth app not found"))?;

        Ok(tonic::Response::new(GetOAuthAppResponse {
            app: Some(to_proto(app)),
        }))
    }

    async fn update_o_auth_app(
        &self,
        request: tonic::Request<UpdateOAuthAppRequest>,
    ) -> Result<tonic::Response<UpdateOAuthAppResponse>, tonic::Status> {
        let actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .into_actor();
        let req = request.into_inner();
        let org_id = self.authorize_admin(&actor, &req.organisation_id).await?;
        let app_id = parse_uuid(&req.app_id, "app_id")?;

        let app = self
            .service()
            .update_app(
                org_id,
                app_id,
                &req.name,
                &req.description,
                &req.homepage_url,
                &req.redirect_uris,
                &req.scopes,
            )
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("oauth app not found"))?;

        Ok(tonic::Response::new(UpdateOAuthAppResponse {
            app: Some(to_proto(app)),
        }))
    }

    async fn rotate_o_auth_app_secret(
        &self,
        request: tonic::Request<RotateOAuthAppSecretRequest>,
    ) -> Result<tonic::Response<RotateOAuthAppSecretResponse>, tonic::Status> {
        let actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .into_actor();
        let req = request.into_inner();
        let org_id = self.authorize_admin(&actor, &req.organisation_id).await?;
        let app_id = parse_uuid(&req.app_id, "app_id")?;

        let created = self
            .service()
            .rotate_secret(org_id, app_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("oauth app not found"))?;

        Ok(tonic::Response::new(RotateOAuthAppSecretResponse {
            app: Some(to_proto(created.app)),
            client_secret: created.client_secret,
        }))
    }

    async fn delete_o_auth_app(
        &self,
        request: tonic::Request<DeleteOAuthAppRequest>,
    ) -> Result<tonic::Response<DeleteOAuthAppResponse>, tonic::Status> {
        let actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .into_actor();
        let req = request.into_inner();
        let org_id = self.authorize_admin(&actor, &req.organisation_id).await?;
        let app_id = parse_uuid(&req.app_id, "app_id")?;

        let deleted = self
            .service()
            .delete_app(org_id, app_id)
            .await
            .map_err(error::to_status)?;
        if !deleted {
            return Err(tonic::Status::not_found("oauth app not found"));
        }

        Ok(tonic::Response::new(DeleteOAuthAppResponse {}))
    }

    // ── Authorization server (service-account only; called by Forage) ──

    async fn lookup_o_auth_client(
        &self,
        request: tonic::Request<LookupOAuthClientRequest>,
    ) -> Result<tonic::Response<LookupOAuthClientResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();

        let app = self
            .service()
            .lookup_client(&req.client_id)
            .await
            .map_err(error::to_status)?
            .ok_or_else(|| tonic::Status::not_found("unknown client"))?;

        Ok(tonic::Response::new(LookupOAuthClientResponse {
            app_id: app.app_id.to_string(),
            organisation_id: app.organisation_id.to_string(),
            name: app.name,
            description: app.description,
            homepage_url: app.homepage_url,
            redirect_uris: app.redirect_uris,
            scopes: app.scopes,
        }))
    }

    async fn create_o_auth_authorization_code(
        &self,
        request: tonic::Request<CreateOAuthAuthorizationCodeRequest>,
    ) -> Result<tonic::Response<CreateOAuthAuthorizationCodeResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();
        let user_id = parse_uuid(&req.user_id, "user_id")?;

        let (code, expires_in_seconds) = self
            .service()
            .create_authorization_code(
                &req.client_id,
                user_id,
                &req.redirect_uri,
                &req.scopes,
                Some(req.code_challenge.as_str()),
                Some(req.code_challenge_method.as_str()),
                Some(req.nonce.as_str()),
            )
            .await
            .map_err(oauth_flow_status)?;

        Ok(tonic::Response::new(CreateOAuthAuthorizationCodeResponse {
            code,
            expires_in_seconds,
        }))
    }

    /// The client_credentials grant. Same service-account gate as the
    /// rest of the authorization-server surface: Forage fronts it, and
    /// the client's own credentials are checked inside the service.
    async fn issue_client_credentials_token(
        &self,
        request: tonic::Request<IssueClientCredentialsTokenRequest>,
    ) -> Result<tonic::Response<IssueClientCredentialsTokenResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();

        let issued = self
            .service()
            .client_credentials_token(&req.client_id, &req.client_secret, &req.scopes)
            .await
            .map_err(oauth_flow_status)?;

        Ok(tonic::Response::new(IssueClientCredentialsTokenResponse {
            access_token: issued.access_token,
            token_type: "bearer".to_string(),
            expires_in_seconds: issued.expires_in_seconds,
            scopes: issued.scopes,
        }))
    }

    /// Resolve a machine token for a resource server.
    ///
    /// An unknown, expired or revoked token all return `active: false`
    /// with empty fields — the caller learns only that it can't be used,
    /// which keeps this from doubling as a token-probing oracle.
    async fn introspect_client_token(
        &self,
        request: tonic::Request<IntrospectClientTokenRequest>,
    ) -> Result<tonic::Response<IntrospectClientTokenResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();

        let found = self
            .service()
            .introspect_client_token(&req.access_token)
            .await
            .map_err(oauth_flow_status)?;

        Ok(tonic::Response::new(match found {
            Some(p) => IntrospectClientTokenResponse {
                active: true,
                app_id: p.app_id.to_string(),
                organisation_id: p.organisation_id.to_string(),
                scopes: p.scopes,
            },
            None => IntrospectClientTokenResponse {
                active: false,
                ..Default::default()
            },
        }))
    }

    async fn exchange_o_auth_code(
        &self,
        request: tonic::Request<ExchangeOAuthCodeRequest>,
    ) -> Result<tonic::Response<ExchangeOAuthCodeResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();

        let tokens = self
            .service()
            .exchange_code(
                &req.client_id,
                &req.client_secret,
                &req.code,
                &req.redirect_uri,
                Some(req.code_verifier.as_str()),
                self.issuer(),
            )
            .await
            .map_err(oauth_flow_status)?;

        Ok(tonic::Response::new(ExchangeOAuthCodeResponse {
            tokens: Some(to_proto_tokens(tokens)),
        }))
    }

    async fn refresh_o_auth_token(
        &self,
        request: tonic::Request<RefreshOAuthTokenRequest>,
    ) -> Result<tonic::Response<RefreshOAuthTokenResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();

        let tokens = self
            .service()
            .refresh_token(
                &req.client_id,
                &req.client_secret,
                &req.refresh_token,
                self.issuer(),
            )
            .await
            .map_err(oauth_flow_status)?;

        Ok(tonic::Response::new(RefreshOAuthTokenResponse {
            tokens: Some(to_proto_tokens(tokens)),
        }))
    }

    async fn revoke_o_auth_grant(
        &self,
        request: tonic::Request<RevokeOAuthGrantRequest>,
    ) -> Result<tonic::Response<RevokeOAuthGrantResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();
        let user_id = parse_uuid(&req.user_id, "user_id")?;
        let app_id = parse_uuid(&req.app_id, "app_id")?;

        let revoked_count = self
            .service()
            .revoke_grant(app_id, user_id)
            .await
            .map_err(error::to_status)? as u32;

        Ok(tonic::Response::new(RevokeOAuthGrantResponse {
            revoked_count,
        }))
    }

    async fn get_o_auth_consent(
        &self,
        request: tonic::Request<GetOAuthConsentRequest>,
    ) -> Result<tonic::Response<GetOAuthConsentResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();
        let user_id = parse_uuid(&req.user_id, "user_id")?;

        let scopes = self
            .service()
            .consented_scopes(&req.client_id, user_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(GetOAuthConsentResponse { scopes }))
    }

    async fn list_o_auth_grants(
        &self,
        request: tonic::Request<ListOAuthGrantsRequest>,
    ) -> Result<tonic::Response<ListOAuthGrantsResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();
        let user_id = parse_uuid(&req.user_id, "user_id")?;

        let grants = self
            .service()
            .list_grants(user_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(ListOAuthGrantsResponse {
            grants: grants
                .into_iter()
                .map(|g| forest_grpc_interface::OAuthGrant {
                    app_id: g.app_id.to_string(),
                    name: g.name,
                    scopes: g.scopes,
                    authorized_at: Some(datetime_to_timestamp(g.authorized_at)),
                })
                .collect(),
        }))
    }

    async fn get_o_auth_userinfo(
        &self,
        request: tonic::Request<GetOAuthUserinfoRequest>,
    ) -> Result<tonic::Response<GetOAuthUserinfoResponse>, tonic::Status> {
        let _actor = authorize::unauthenticated_actor(&request)
            .require_authenticated()?
            .require_service_account()?;
        let req = request.into_inner();

        let info = self
            .service()
            .userinfo(&req.access_token)
            .await
            .map_err(oauth_flow_status)?;

        Ok(tonic::Response::new(GetOAuthUserinfoResponse {
            userinfo: Some(OAuthUserinfo {
                sub: info.sub,
                username: info.username.unwrap_or_default(),
                profile_picture_url: info.profile_picture_url.unwrap_or_default(),
                email: info.email.unwrap_or_default(),
                emails: info.emails,
                scopes: info.scopes,
            }),
        }))
    }
}

/// Map authorization-server flow errors to gRPC statuses whose *message* is the
/// RFC 6749 error code, so Forage can translate them into OAuth error responses
/// (`invalid_client` → 401, `invalid_grant`/`invalid_scope` → 400). Falls back
/// to the shared mapper for non-flow errors.
fn oauth_flow_status(err: anyhow::Error) -> tonic::Status {
    use crate::services::oauth_apps::OAuthAppError;
    if let Some(e) = err.downcast_ref::<OAuthAppError>() {
        return match e {
            OAuthAppError::UnknownClient | OAuthAppError::InvalidClientSecret => {
                tonic::Status::unauthenticated("invalid_client")
            }
            OAuthAppError::InvalidGrant | OAuthAppError::PkceFailed => {
                tonic::Status::failed_precondition("invalid_grant")
            }
            OAuthAppError::RedirectUriNotAllowed => {
                tonic::Status::invalid_argument("invalid_request")
            }
            OAuthAppError::UnknownScope(_) | OAuthAppError::NoScopes => {
                tonic::Status::invalid_argument("invalid_scope")
            }
            OAuthAppError::InvalidCodeChallengeMethod => {
                tonic::Status::invalid_argument("invalid_request")
            }
            // RFC 6749 names these exactly.
            OAuthAppError::UnsupportedGrant(_) => {
                tonic::Status::invalid_argument("unsupported_grant_type")
            }
            OAuthAppError::ScopeNotGranted(_) => {
                tonic::Status::invalid_argument("invalid_scope")
            }
            OAuthAppError::InvalidName
            | OAuthAppError::NoRedirectUris
            | OAuthAppError::InvalidRedirectUri(_) => {
                tonic::Status::invalid_argument(e.to_string())
            }
        };
    }
    error::to_status(err)
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, tonic::Status> {
    value
        .parse::<Uuid>()
        .map_err(|_| tonic::Status::invalid_argument(format!("invalid {field}")))
}

/// OAuth-app management is performed by a logged-in org admin (a user). The
/// service-account bypass in `require_org_access_by_id` has no `created_by`,
/// so creation specifically requires a user actor.
fn acting_user_id(actor: &Actor) -> Result<Uuid, tonic::Status> {
    match actor {
        Actor::User { user_id } => Ok(*user_id),
        _ => Err(tonic::Status::permission_denied(
            "oauth app creation must be performed by a user",
        )),
    }
}

fn to_proto(app: OAuthApp) -> forest_grpc_interface::OAuthApp {
    forest_grpc_interface::OAuthApp {
        app_id: app.app_id.to_string(),
        organisation_id: app.organisation_id.to_string(),
        name: app.name,
        description: app.description,
        homepage_url: app.homepage_url,
        client_id: app.client_id,
        redirect_uris: app.redirect_uris,
        grant_types: app.grant_types,
        scopes: app.scopes,
        created_by: app.created_by.to_string(),
        created_at: Some(datetime_to_timestamp(app.created_at)),
        updated_at: Some(datetime_to_timestamp(app.updated_at)),
    }
}

fn to_proto_tokens(tokens: crate::services::oauth_apps::IssuedTokens) -> OAuthTokens {
    OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        token_type: "bearer".into(),
        expires_in_seconds: tokens.expires_in_seconds,
        scopes: tokens.scopes,
        id_token: tokens.id_token.unwrap_or_default(),
    }
}

fn datetime_to_timestamp(dt: chrono::DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}
