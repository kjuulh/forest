use sha2::Digest;
use uuid::Uuid;

use crate::{
    State,
    repositories::oauth_apps::{OAuthAppRepository, OAuthAppRepositoryState, OAuthAppRow},
    services::{
        device_login_aggregate::{
            DeviceLoginAggregateService, DeviceLoginPollOutcome, InitiatedDeviceGrant,
        },
        organisations::{OrganisationService, OrganisationServiceState},
    },
};

pub const ALLOWED_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    SCOPE_DIRECTORY_READ,
    SCOPE_COMPONENTS_PUBLISH,
    SCOPE_TOKENS_INTROSPECT,
    SCOPE_DATAGATEWAY_APPLY,
    SCOPE_DATAGATEWAY_READ,
];

pub const SCOPE_TOKENS_INTROSPECT: &str = "tokens:introspect";

pub const SCOPE_DATAGATEWAY_APPLY: &str = "datagateway:apply";
pub const SCOPE_DATAGATEWAY_READ: &str = "datagateway:read";

pub const SCOPE_DIRECTORY_READ: &str = "directory:read";

pub const SCOPE_COMPONENTS_PUBLISH: &str = "components:publish";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthAppError {
    #[error("name must be between 1 and 100 characters")]
    InvalidName,
    #[error("at least one redirect URI is required")]
    NoRedirectUris,
    #[error("invalid redirect URI: {0}")]
    InvalidRedirectUri(String),
    #[error("unknown scope: {0}")]
    UnknownScope(String),
    #[error("at least one scope is required")]
    NoScopes,

    #[error("unknown client")]
    UnknownClient,
    #[error("invalid client credentials")]
    InvalidClientSecret,
    #[error("client is not authorised for the {0} grant")]
    UnsupportedGrant(String),
    #[error("scope not registered for this client: {0}")]
    ScopeNotGranted(String),
    #[error("redirect_uri not registered for this client")]
    RedirectUriNotAllowed,
    #[error("invalid or expired authorization code")]
    InvalidGrant,
    #[error("PKCE verification failed")]
    PkceFailed,
    #[error("unsupported code_challenge_method")]
    InvalidCodeChallengeMethod,

    #[error("a public client may only use the authorization_code and device_code grants")]
    PublicClientGrant,
    #[error("a public client may not request the openid scope")]
    PublicClientOpenid,
    #[error("a public client has no client secret to rotate")]
    PublicClientNoSecret,
    #[error("a public client must use PKCE with code_challenge_method S256")]
    PkceRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthApp {
    pub app_id: Uuid,
    pub organisation_id: Uuid,
    pub name: String,
    pub description: String,
    pub homepage_url: String,
    pub client_id: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub grant_types: Vec<String>,
    pub public: bool,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<OAuthAppRow> for OAuthApp {
    fn from(row: OAuthAppRow) -> Self {
        OAuthApp {
            app_id: row.id,
            organisation_id: row.organisation_id,
            name: row.name,
            description: row.description,
            homepage_url: row.homepage_url,
            client_id: row.client_id,
            redirect_uris: row.redirect_uris,
            scopes: row.scopes,
            grant_types: row.grant_types,
            public: row.public,
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreatedOAuthApp {
    pub app: OAuthApp,
    pub client_secret: String,
}

pub const AUTH_CODE_TTL_SECONDS: i64 = 60;
pub const ACCESS_TOKEN_TTL_SECONDS: i64 = 8 * 3600;
pub const CLIENT_TOKEN_TTL_SECONDS: i64 = 3600;

pub const GRANT_AUTHORIZATION_CODE: &str = "authorization_code";
pub const GRANT_CLIENT_CREDENTIALS: &str = "client_credentials";
pub const GRANT_DEVICE_CODE: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub const SUPPORTED_GRANT_TYPES: &[&str] = &[
    GRANT_AUTHORIZATION_CODE,
    GRANT_CLIENT_CREDENTIALS,
    GRANT_DEVICE_CODE,
];

const USER_GRANTS: &[&str] = &[GRANT_AUTHORIZATION_CODE, GRANT_DEVICE_CODE];
pub const REFRESH_TOKEN_TTL_SECONDS: i64 = 90 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedClientToken {
    pub access_token: String,
    pub expires_in_seconds: i64,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPrincipal {
    pub app_id: Uuid,
    pub organisation_id: Uuid,
    pub scopes: Vec<String>,
    pub user: Option<DelegatingUser>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatingUser {
    pub user_id: Uuid,
    pub username: String,
    pub organisations: Vec<UserOrganisation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserOrganisation {
    pub organisation_id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: i64,
    pub scopes: Vec<String>,
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTokenOutcome {
    Issued(IssuedTokens),
    AuthorizationPending,
    SlowDown,
    AccessDenied,
    ExpiredToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDeviceAuthorization {
    pub app: OAuthApp,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrant {
    pub app_id: uuid::Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub authorized_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Userinfo {
    pub sub: String,
    pub username: Option<String>,
    pub profile_picture_url: Option<String>,
    pub email: Option<String>,
    pub emails: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug)]
pub struct OAuthAppInput {
    pub name: String,
    pub description: String,
    pub homepage_url: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
}

pub struct OAuthAppService {
    repo: OAuthAppRepository,
    organisations: OrganisationService,
}

impl OAuthAppService {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_app(
        &self,
        organisation_id: Uuid,
        created_by: Uuid,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
        public: bool,
    ) -> anyhow::Result<CreatedOAuthApp> {
        let input = validate_input(name, description, homepage_url, redirect_uris, scopes)?;
        let grant_types = validate_grant_types(grant_types)?;
        if public {
            validate_public_client(&input.scopes, &grant_types)?;
        }

        let app_id = Uuid::now_v7();
        let client_id = generate_client_id();
        let client_secret = generate_client_secret();
        let secret_hash = hash_secret(&client_secret);
        let client_secret = if public { String::new() } else { client_secret };

        let row = self
            .repo
            .create_oauth_app(
                self.repo.pool(),
                app_id,
                organisation_id,
                &input.name,
                &input.description,
                &input.homepage_url,
                &client_id,
                &secret_hash,
                &input.redirect_uris,
                &input.scopes,
                &grant_types,
                public,
                created_by,
            )
            .await?;

        Ok(CreatedOAuthApp {
            app: row.into(),
            client_secret,
        })
    }

    pub async fn list_apps(&self, organisation_id: Uuid) -> anyhow::Result<Vec<OAuthApp>> {
        let rows = self
            .repo
            .list_oauth_apps(self.repo.pool(), organisation_id)
            .await?;
        Ok(rows.into_iter().map(OAuthApp::from).collect())
    }

    pub async fn get_app(
        &self,
        organisation_id: Uuid,
        app_id: Uuid,
    ) -> anyhow::Result<Option<OAuthApp>> {
        let row = self
            .repo
            .get_oauth_app(self.repo.pool(), organisation_id, app_id)
            .await?;
        Ok(row.map(OAuthApp::from))
    }

    pub async fn update_app(
        &self,
        organisation_id: Uuid,
        app_id: Uuid,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
    ) -> anyhow::Result<Option<OAuthApp>> {
        let input = validate_input(name, description, homepage_url, redirect_uris, scopes)?;
        let grant_types = validate_grant_types(grant_types)?;

        let Some(existing) = self
            .repo
            .get_oauth_app(self.repo.pool(), organisation_id, app_id)
            .await?
        else {
            return Ok(None);
        };
        if existing.public {
            validate_public_client(&input.scopes, &grant_types)?;
        }

        let row = self
            .repo
            .update_oauth_app(
                self.repo.pool(),
                organisation_id,
                app_id,
                &input.name,
                &input.description,
                &input.homepage_url,
                &input.redirect_uris,
                &input.scopes,
                &grant_types,
            )
            .await?;
        Ok(row.map(OAuthApp::from))
    }

    pub async fn rotate_secret(
        &self,
        organisation_id: Uuid,
        app_id: Uuid,
    ) -> anyhow::Result<Option<CreatedOAuthApp>> {
        let Some(existing) = self
            .repo
            .get_oauth_app(self.repo.pool(), organisation_id, app_id)
            .await?
        else {
            return Ok(None);
        };
        if existing.public {
            return Err(OAuthAppError::PublicClientNoSecret.into());
        }

        let client_secret = generate_client_secret();
        let secret_hash = hash_secret(&client_secret);

        let row = self
            .repo
            .rotate_oauth_app_secret(self.repo.pool(), organisation_id, app_id, &secret_hash)
            .await?;
        Ok(row.map(|row| CreatedOAuthApp {
            app: row.into(),
            client_secret,
        }))
    }

    pub async fn delete_app(&self, organisation_id: Uuid, app_id: Uuid) -> anyhow::Result<bool> {
        self.repo
            .delete_oauth_app(self.repo.pool(), organisation_id, app_id)
            .await
    }

    pub async fn lookup_client(&self, client_id: &str) -> anyhow::Result<Option<OAuthApp>> {
        let row = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?;
        Ok(row.map(OAuthApp::from))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_authorization_code(
        &self,
        client_id: &str,
        user_id: Uuid,
        redirect_uri: &str,
        scopes: &[String],
        code_challenge: Option<&str>,
        code_challenge_method: Option<&str>,
        nonce: Option<&str>,
    ) -> anyhow::Result<(String, i64)> {
        let app = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?
            .ok_or(OAuthAppError::UnknownClient)?;

        Self::require_grant(&app, GRANT_AUTHORIZATION_CODE)?;

        if !app
            .redirect_uris
            .iter()
            .any(|u| redirect_uri_matches(u, redirect_uri))
        {
            return Err(OAuthAppError::RedirectUriNotAllowed.into());
        }
        if scopes.is_empty() {
            return Err(OAuthAppError::NoScopes.into());
        }
        for scope in scopes {
            if !app.scopes.contains(scope) {
                return Err(OAuthAppError::UnknownScope(scope.clone()).into());
            }
        }

        let challenge = non_empty(code_challenge);
        let method = normalize_pkce_method(challenge, non_empty(code_challenge_method))?;
        if app.public && method.as_deref() != Some("S256") {
            return Err(OAuthAppError::PkceRequired.into());
        }

        let (code, code_hash) = generate_opaque_token("forest_ac_");
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(AUTH_CODE_TTL_SECONDS);

        self.repo
            .create_authorization_code(
                self.repo.pool(),
                &code_hash,
                app.id,
                user_id,
                redirect_uri,
                scopes,
                challenge,
                method.as_deref(),
                non_empty(nonce),
                expires_at,
            )
            .await?;

        self.repo
            .upsert_consent(self.repo.pool(), app.id, user_id, scopes)
            .await?;

        Ok((code, AUTH_CODE_TTL_SECONDS))
    }

    pub async fn consented_scopes(
        &self,
        client_id: &str,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<String>> {
        Ok(self
            .repo
            .get_consent_by_client(self.repo.pool(), client_id, user_id)
            .await?
            .unwrap_or_default())
    }

    pub async fn exchange_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
        issuer: &str,
    ) -> anyhow::Result<IssuedTokens> {
        let row = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?
            .ok_or(OAuthAppError::UnknownClient)?;

        Self::authenticate_client(&row, client_secret)?;

        Self::require_grant(&row, GRANT_AUTHORIZATION_CODE)?;

        let code_hash = hash_secret(code);
        let consumed = self
            .repo
            .consume_authorization_code(self.repo.pool(), &code_hash)
            .await?
            .ok_or(OAuthAppError::InvalidGrant)?;

        if consumed.app_id != row.id || consumed.redirect_uri != redirect_uri {
            return Err(OAuthAppError::InvalidGrant.into());
        }

        verify_pkce(
            consumed.code_challenge.as_deref(),
            consumed.code_challenge_method.as_deref(),
            code_verifier,
        )?;

        let (access_token, access_hash) = generate_opaque_token("forest_oat_");
        let (refresh_token, refresh_hash) = generate_opaque_token("forest_ort_");
        let now = chrono::Utc::now();
        let access_exp = now + chrono::Duration::seconds(ACCESS_TOKEN_TTL_SECONDS);
        let refresh_exp = now + chrono::Duration::seconds(REFRESH_TOKEN_TTL_SECONDS);

        self.repo
            .insert_access_token(
                self.repo.pool(),
                &access_hash,
                row.id,
                consumed.user_id,
                &consumed.scopes,
                Some(&refresh_hash),
                access_exp,
                Some(refresh_exp),
            )
            .await?;

        let id_token = self
            .maybe_id_token(
                &row,
                issuer,
                client_secret,
                consumed.user_id,
                &consumed.scopes,
                consumed.nonce.as_deref(),
            )
            .await?;

        Ok(IssuedTokens {
            access_token,
            refresh_token,
            expires_in_seconds: ACCESS_TOKEN_TTL_SECONDS,
            scopes: consumed.scopes,
            id_token,
        })
    }

    async fn maybe_id_token(
        &self,
        app: &OAuthAppRow,
        issuer: &str,
        client_secret: &str,
        user_id: Uuid,
        scopes: &[String],
        nonce: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        if app.public || !scopes.iter().any(|s| s == "openid") {
            return Ok(None);
        }
        let client_id = app.client_id.as_str();
        let username = if scopes.iter().any(|s| s == "profile") {
            self.repo
                .get_user_basic(self.repo.pool(), user_id)
                .await?
                .map(|(u, _)| u)
        } else {
            None
        };
        let email = if scopes.iter().any(|s| s == "email") {
            self.repo
                .get_verified_emails(self.repo.pool(), user_id)
                .await?
                .into_iter()
                .next()
        } else {
            None
        };
        let now = chrono::Utc::now().timestamp();
        let exp = now + ACCESS_TOKEN_TTL_SECONDS;
        let token = mint_id_token(
            client_secret,
            issuer,
            client_id,
            &user_id.to_string(),
            username.as_deref(),
            email.as_deref(),
            nonce,
            now,
            exp,
        )?;
        Ok(Some(token))
    }

    fn authenticate_client(app: &OAuthAppRow, client_secret: &str) -> Result<(), OAuthAppError> {
        if app.public {
            return Ok(());
        }
        if !constant_time_eq(&hash_secret(client_secret), &app.client_secret_hash) {
            return Err(OAuthAppError::InvalidClientSecret);
        }
        Ok(())
    }

    fn require_grant(app: &OAuthAppRow, grant: &str) -> Result<(), OAuthAppError> {
        if app.grant_types.iter().any(|g| g == grant) {
            return Ok(());
        }
        Err(OAuthAppError::UnsupportedGrant(grant.to_string()))
    }

    pub async fn client_credentials_token(
        &self,
        client_id: &str,
        client_secret: &str,
        requested_scopes: &[String],
    ) -> anyhow::Result<IssuedClientToken> {
        let row = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?
            .ok_or(OAuthAppError::UnknownClient)?;

        if !constant_time_eq(&hash_secret(client_secret), &row.client_secret_hash) {
            return Err(OAuthAppError::InvalidClientSecret.into());
        }

        Self::require_grant(&row, GRANT_CLIENT_CREDENTIALS)?;

        let scopes = if requested_scopes.is_empty() {
            row.scopes.clone()
        } else {
            for requested in requested_scopes {
                if !row.scopes.iter().any(|s| s == requested) {
                    return Err(OAuthAppError::ScopeNotGranted(requested.clone()).into());
                }
            }
            requested_scopes.to_vec()
        };

        let (access_token, access_hash) = generate_opaque_token("forest_cat_");
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(CLIENT_TOKEN_TTL_SECONDS);
        self.repo
            .insert_client_token(self.repo.pool(), &access_hash, row.id, &scopes, expires_at)
            .await?;

        Ok(IssuedClientToken {
            access_token,
            expires_in_seconds: CLIENT_TOKEN_TTL_SECONDS,
            scopes,
        })
    }

    pub async fn introspect_client_token(
        &self,
        access_token: &str,
    ) -> anyhow::Result<Option<ClientPrincipal>> {
        let hash = hash_secret(access_token);
        let Some(row) = self
            .repo
            .resolve_client_token(self.repo.pool(), &hash)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(ClientPrincipal {
            app_id: row.app_id,
            organisation_id: row.organisation_id,
            scopes: row.scopes,
            user: None,
        }))
    }

    pub async fn introspect_token(
        &self,
        access_token: &str,
    ) -> anyhow::Result<Option<ClientPrincipal>> {
        if let Some(principal) = self.introspect_client_token(access_token).await? {
            return Ok(Some(principal));
        }

        if !access_token.starts_with(USER_ACCESS_TOKEN_PREFIX) {
            return Ok(None);
        }
        let Some(row) = self
            .repo
            .resolve_user_token(self.repo.pool(), &hash_secret(access_token))
            .await?
        else {
            return Ok(None);
        };
        let organisations = self
            .organisations
            .list_my_organisations(row.user_id, None)
            .await?
            .into_iter()
            .map(|o| UserOrganisation {
                organisation_id: o.organisation_id,
                name: o.name,
            })
            .collect();
        Ok(Some(ClientPrincipal {
            app_id: row.app_id,
            organisation_id: row.organisation_id,
            scopes: row.scopes,
            user: Some(DelegatingUser {
                user_id: row.user_id,
                username: row.username,
                organisations,
            }),
        }))
    }

    pub async fn refresh_token(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
        issuer: &str,
    ) -> anyhow::Result<IssuedTokens> {
        let app = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?
            .ok_or(OAuthAppError::UnknownClient)?;
        Self::authenticate_client(&app, client_secret)?;
        if !app
            .grant_types
            .iter()
            .any(|g| USER_GRANTS.contains(&g.as_str()))
        {
            return Err(OAuthAppError::UnsupportedGrant(GRANT_AUTHORIZATION_CODE.into()).into());
        }

        let refresh_hash = hash_secret(refresh_token);

        let consumed = self
            .repo
            .consume_refresh_token(self.repo.pool(), &refresh_hash, app.id)
            .await?;

        let consumed = match consumed {
            Some(row) => row,
            None => {
                if let Some(existing) = self
                    .repo
                    .find_token_by_refresh(self.repo.pool(), &refresh_hash)
                    .await?
                    && existing.revoked_at.is_some()
                    && existing.app_id == app.id
                {
                    self.repo
                        .revoke_user_app_tokens(self.repo.pool(), existing.app_id, existing.user_id)
                        .await?;
                }
                return Err(OAuthAppError::InvalidGrant.into());
            }
        };

        let (access_token, access_hash) = generate_opaque_token("forest_oat_");
        let (new_refresh, new_refresh_hash) = generate_opaque_token("forest_ort_");
        let now = chrono::Utc::now();
        self.repo
            .insert_access_token(
                self.repo.pool(),
                &access_hash,
                app.id,
                consumed.user_id,
                &consumed.scopes,
                Some(&new_refresh_hash),
                now + chrono::Duration::seconds(ACCESS_TOKEN_TTL_SECONDS),
                Some(now + chrono::Duration::seconds(REFRESH_TOKEN_TTL_SECONDS)),
            )
            .await?;

        let id_token = self
            .maybe_id_token(
                &app,
                issuer,
                client_secret,
                consumed.user_id,
                &consumed.scopes,
                None,
            )
            .await?;

        Ok(IssuedTokens {
            access_token,
            refresh_token: new_refresh,
            expires_in_seconds: ACCESS_TOKEN_TTL_SECONDS,
            scopes: consumed.scopes,
            id_token,
        })
    }

    pub async fn initiate_device_authorization(
        &self,
        devices: &DeviceLoginAggregateService,
        client_id: &str,
        client_secret: &str,
        scopes: &[String],
    ) -> anyhow::Result<InitiatedDeviceGrant> {
        let app = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?
            .ok_or(OAuthAppError::UnknownClient)?;
        Self::authenticate_client(&app, client_secret)?;
        Self::require_grant(&app, GRANT_DEVICE_CODE)?;
        if scopes.is_empty() {
            return Err(OAuthAppError::NoScopes.into());
        }
        let mut granted: Vec<String> = Vec::new();
        for scope in scopes {
            if !app.scopes.contains(scope) {
                return Err(OAuthAppError::UnknownScope(scope.clone()).into());
            }
            if !granted.contains(scope) {
                granted.push(scope.clone());
            }
        }
        devices.initiate(&app.name, "", granted, Some(app.id)).await
    }

    pub async fn pending_device_authorization(
        &self,
        devices: &DeviceLoginAggregateService,
        user_code: &str,
    ) -> anyhow::Result<Option<PendingDeviceAuthorization>> {
        let Some(pending) = devices.pending(user_code).await? else {
            return Ok(None);
        };
        let Some(app_id) = pending.app_id else {
            return Ok(None);
        };
        let Some(app) = self
            .repo
            .get_oauth_app_by_id(self.repo.pool(), app_id)
            .await?
        else {
            return Ok(None);
        };
        if Self::require_grant(&app, GRANT_DEVICE_CODE).is_err()
            || pending.scopes.iter().any(|s| !app.scopes.contains(s))
        {
            return Ok(None);
        }
        Ok(Some(PendingDeviceAuthorization {
            app: app.into(),
            scopes: pending.scopes,
        }))
    }

    pub async fn approve_device_authorization(
        &self,
        devices: &DeviceLoginAggregateService,
        user_code: &str,
        user_id: Uuid,
        approving_ip: &str,
        approving_user_agent: &str,
    ) -> anyhow::Result<()> {
        let pending = self
            .pending_device_authorization(devices, user_code)
            .await?
            .ok_or(OAuthAppError::InvalidGrant)?;
        devices
            .approve(
                user_code,
                user_id,
                approving_ip,
                approving_user_agent,
                Some(pending.app.app_id),
            )
            .await?;
        self.repo
            .upsert_consent(
                self.repo.pool(),
                pending.app.app_id,
                user_id,
                &pending.scopes,
            )
            .await
    }

    pub async fn deny_device_authorization(
        &self,
        devices: &DeviceLoginAggregateService,
        user_code: &str,
        user_id: Uuid,
    ) -> anyhow::Result<()> {
        let pending = devices
            .pending(user_code)
            .await?
            .and_then(|p| p.app_id)
            .ok_or(OAuthAppError::InvalidGrant)?;
        devices.deny(user_code, user_id, Some(pending)).await
    }

    pub async fn exchange_device_code(
        &self,
        devices: &DeviceLoginAggregateService,
        client_id: &str,
        client_secret: &str,
        device_code: &str,
    ) -> anyhow::Result<DeviceTokenOutcome> {
        let app = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?
            .ok_or(OAuthAppError::UnknownClient)?;
        Self::authenticate_client(&app, client_secret)?;
        Self::require_grant(&app, GRANT_DEVICE_CODE)?;

        let (user_id, scopes) = match devices.poll(device_code, Some(app.id)).await? {
            DeviceLoginPollOutcome::Pending => return Ok(DeviceTokenOutcome::AuthorizationPending),
            DeviceLoginPollOutcome::SlowDown => return Ok(DeviceTokenOutcome::SlowDown),
            DeviceLoginPollOutcome::Denied => return Ok(DeviceTokenOutcome::AccessDenied),
            DeviceLoginPollOutcome::Expired => return Ok(DeviceTokenOutcome::ExpiredToken),
            DeviceLoginPollOutcome::Approved { user_id, scopes } => (user_id, scopes),
        };
        if scopes.is_empty() || scopes.iter().any(|s| !app.scopes.contains(s)) {
            return Err(OAuthAppError::InvalidGrant.into());
        }

        let (access_token, access_hash) = generate_opaque_token(USER_ACCESS_TOKEN_PREFIX);
        let (refresh_token, refresh_hash) = generate_opaque_token("forest_ort_");
        let now = chrono::Utc::now();
        self.repo
            .insert_access_token(
                self.repo.pool(),
                &access_hash,
                app.id,
                user_id,
                &scopes,
                Some(&refresh_hash),
                now + chrono::Duration::seconds(ACCESS_TOKEN_TTL_SECONDS),
                Some(now + chrono::Duration::seconds(REFRESH_TOKEN_TTL_SECONDS)),
            )
            .await?;

        Ok(DeviceTokenOutcome::Issued(IssuedTokens {
            access_token,
            refresh_token,
            expires_in_seconds: ACCESS_TOKEN_TTL_SECONDS,
            scopes,
            id_token: None,
        }))
    }

    pub async fn list_grants(&self, user_id: Uuid) -> anyhow::Result<Vec<OAuthGrant>> {
        let rows = self
            .repo
            .list_live_grant_rows(self.repo.pool(), user_id)
            .await?;

        let mut grants: Vec<OAuthGrant> = Vec::new();
        for row in rows {
            if let Some(existing) = grants.iter_mut().find(|g| g.app_id == row.app_id) {
                for scope in row.scopes {
                    if !existing.scopes.contains(&scope) {
                        existing.scopes.push(scope);
                    }
                }
            } else {
                grants.push(OAuthGrant {
                    app_id: row.app_id,
                    name: row.name,
                    scopes: row.scopes,
                    authorized_at: row.created_at,
                });
            }
        }
        Ok(grants)
    }

    pub async fn revoke_grant(&self, app_id: Uuid, user_id: Uuid) -> anyhow::Result<u64> {
        let revoked = self
            .repo
            .revoke_user_app_tokens(self.repo.pool(), app_id, user_id)
            .await?;
        self.repo
            .delete_consent(self.repo.pool(), app_id, user_id)
            .await?;
        Ok(revoked)
    }

    pub async fn userinfo(&self, access_token: &str) -> anyhow::Result<Userinfo> {
        let token_hash = hash_secret(access_token);
        let resolved = self
            .repo
            .resolve_access_token(self.repo.pool(), &token_hash)
            .await?
            .ok_or(OAuthAppError::InvalidGrant)?;

        let scopes = resolved.scopes;
        let mut info = Userinfo {
            sub: resolved.user_id.to_string(),
            username: None,
            profile_picture_url: None,
            email: None,
            emails: Vec::new(),
            scopes: scopes.clone(),
        };

        if scopes.iter().any(|s| s == "profile")
            && let Some((username, picture)) = self
                .repo
                .get_user_basic(self.repo.pool(), resolved.user_id)
                .await?
        {
            info.username = Some(username);
            info.profile_picture_url = picture;
        }

        if scopes.iter().any(|s| s == "email") {
            let emails = self
                .repo
                .get_verified_emails(self.repo.pool(), resolved.user_id)
                .await?;
            info.email = emails.first().cloned();
            info.emails = emails;
        }

        Ok(info)
    }
}

pub fn validate_input(
    name: &str,
    description: &str,
    homepage_url: &str,
    redirect_uris: &[String],
    scopes: &[String],
) -> Result<OAuthAppInput, OAuthAppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(OAuthAppError::InvalidName);
    }

    if redirect_uris.is_empty() {
        return Err(OAuthAppError::NoRedirectUris);
    }
    let mut normalized_uris = Vec::with_capacity(redirect_uris.len());
    for uri in redirect_uris {
        let uri = uri.trim();
        validate_redirect_uri(uri)?;
        if !normalized_uris.contains(&uri.to_string()) {
            normalized_uris.push(uri.to_string());
        }
    }

    if scopes.is_empty() {
        return Err(OAuthAppError::NoScopes);
    }
    let mut normalized_scopes = Vec::with_capacity(scopes.len());
    for scope in scopes {
        let scope = scope.trim();
        if !ALLOWED_SCOPES.contains(&scope) {
            return Err(OAuthAppError::UnknownScope(scope.to_string()));
        }
        if !normalized_scopes.contains(&scope.to_string()) {
            normalized_scopes.push(scope.to_string());
        }
    }

    Ok(OAuthAppInput {
        name: name.to_string(),
        description: description.trim().to_string(),
        homepage_url: homepage_url.trim().to_string(),
        redirect_uris: normalized_uris,
        scopes: normalized_scopes,
    })
}

pub fn validate_public_client(
    scopes: &[String],
    grant_types: &[String],
) -> Result<(), OAuthAppError> {
    if grant_types
        .iter()
        .any(|g| g != GRANT_AUTHORIZATION_CODE && g != GRANT_DEVICE_CODE)
    {
        return Err(OAuthAppError::PublicClientGrant);
    }
    if scopes.iter().any(|s| s == "openid") {
        return Err(OAuthAppError::PublicClientOpenid);
    }
    Ok(())
}

pub fn validate_grant_types(requested: &[String]) -> Result<Vec<String>, OAuthAppError> {
    if requested.is_empty() {
        return Ok(vec![GRANT_AUTHORIZATION_CODE.to_string()]);
    }
    let mut out: Vec<String> = Vec::new();
    for raw in requested {
        let g = raw.trim().to_ascii_lowercase();
        if !SUPPORTED_GRANT_TYPES.contains(&g.as_str()) {
            return Err(OAuthAppError::UnsupportedGrant(g));
        }
        if !out.contains(&g) {
            out.push(g);
        }
    }
    Ok(out)
}

pub fn validate_redirect_uri(uri: &str) -> Result<(), OAuthAppError> {
    let invalid = |u: &str| OAuthAppError::InvalidRedirectUri(u.to_string());

    if uri.contains('#') {
        return Err(invalid(uri));
    }

    let (scheme, rest) = uri.split_once("://").ok_or_else(|| invalid(uri))?;
    if rest.is_empty() {
        return Err(invalid(uri));
    }

    let host = rest
        .split(['/', '?'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    let is_loopback = host == "localhost" || host == "127.0.0.1";

    match scheme {
        "https" => Ok(()),
        "http" if is_loopback => Ok(()),
        _ => Err(invalid(uri)),
    }
}

pub fn redirect_uri_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    match (loopback_parts(registered), loopback_parts(requested)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn loopback_parts(uri: &str) -> Option<(&str, String)> {
    let rest = uri.strip_prefix("http://")?;
    if rest.contains('#') {
        return None;
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if host != "127.0.0.1" && host != "localhost" {
        return None;
    }
    if let Some(port) = port
        && (port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let tail = if tail.starts_with('/') {
        tail.to_string()
    } else {
        format!("/{tail}")
    };
    Some((host, tail))
}

pub const USER_ACCESS_TOKEN_PREFIX: &str = "forest_oat_";

pub fn generate_client_id() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes[..]);
    format!("forest_oa_{}", hex::encode(bytes))
}

pub fn generate_client_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes[..]);
    format!("forest_oas_{}", hex::encode(bytes))
}

pub fn hash_secret(secret: &str) -> Vec<u8> {
    sha2::Sha256::digest(secret.as_bytes()).to_vec()
}

pub fn generate_opaque_token(prefix: &str) -> (String, Vec<u8>) {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes[..]);
    let raw = format!("{prefix}{}", hex::encode(bytes));
    let hash = sha2::Sha256::digest(raw.as_bytes()).to_vec();
    (raw, hash)
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

fn normalize_pkce_method(
    challenge: Option<&str>,
    method: Option<&str>,
) -> Result<Option<String>, OAuthAppError> {
    match challenge {
        None => Ok(None),
        Some(_) => match method.unwrap_or("plain") {
            "S256" => Ok(Some("S256".to_string())),
            "plain" => Ok(Some("plain".to_string())),
            _ => Err(OAuthAppError::InvalidCodeChallengeMethod),
        },
    }
}

pub fn verify_pkce(
    challenge: Option<&str>,
    method: Option<&str>,
    verifier: Option<&str>,
) -> Result<(), OAuthAppError> {
    let Some(challenge) = challenge else {
        return Ok(());
    };
    let verifier = verifier.ok_or(OAuthAppError::PkceFailed)?;
    let computed = match method.unwrap_or("plain") {
        "plain" => verifier.to_string(),
        "S256" => {
            use base64::Engine;
            let digest = sha2::Sha256::digest(verifier.as_bytes());
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
        }
        _ => return Err(OAuthAppError::InvalidCodeChallengeMethod),
    };
    if constant_time_eq(computed.as_bytes(), challenge.as_bytes()) {
        Ok(())
    } else {
        Err(OAuthAppError::PkceFailed)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn mint_id_token(
    client_secret: &str,
    issuer: &str,
    client_id: &str,
    sub: &str,
    username: Option<&str>,
    email: Option<&str>,
    nonce: Option<&str>,
    iat: i64,
    exp: i64,
) -> anyhow::Result<String> {
    use hmac::{Hmac, Mac};
    use jwt::SignWithKey;
    use sha2::Sha256;
    use std::collections::BTreeMap;

    let key: Hmac<Sha256> = Hmac::new_from_slice(client_secret.as_bytes())
        .map_err(|e| anyhow::anyhow!("id_token key: {e}"))?;
    let header = jwt::Header {
        algorithm: jwt::AlgorithmType::Hs256,
        ..Default::default()
    };
    let iat = iat.to_string();
    let exp = exp.to_string();
    let mut claims: BTreeMap<&str, &str> = BTreeMap::new();
    claims.insert("iss", issuer);
    claims.insert("sub", sub);
    claims.insert("aud", client_id);
    claims.insert("iat", &iat);
    claims.insert("exp", &exp);
    if let Some(u) = username {
        claims.insert("preferred_username", u);
    }
    if let Some(e) = email {
        claims.insert("email", e);
    }
    if let Some(n) = nonce {
        claims.insert("nonce", n);
    }
    let token = jwt::Token::new(header, claims)
        .sign_with_key(&key)
        .map_err(|e| anyhow::anyhow!("id_token sign: {e}"))?;
    Ok(token.as_str().to_string())
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub trait OAuthAppServiceState {
    fn oauth_app_service(&self) -> OAuthAppService;
}

impl OAuthAppServiceState for State {
    fn oauth_app_service(&self) -> OAuthAppService {
        OAuthAppService {
            repo: self.oauth_app_repository(),
            organisations: self.organisation_service(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_overlong_names() {
        let uris = vec!["https://app.example/cb".to_string()];
        let scopes = vec!["profile".to_string()];
        assert_eq!(
            validate_input("", "", "", &uris, &scopes).unwrap_err(),
            OAuthAppError::InvalidName
        );
        let long = "x".repeat(101);
        assert_eq!(
            validate_input(&long, "", "", &uris, &scopes).unwrap_err(),
            OAuthAppError::InvalidName
        );
    }

    #[test]
    fn requires_at_least_one_redirect_uri() {
        assert_eq!(
            validate_input("App", "", "", &[], &["profile".to_string()]).unwrap_err(),
            OAuthAppError::NoRedirectUris
        );
    }

    #[test]
    fn requires_at_least_one_scope() {
        let uris = vec!["https://app.example/cb".to_string()];
        assert_eq!(
            validate_input("App", "", "", &uris, &[]).unwrap_err(),
            OAuthAppError::NoScopes
        );
    }

    #[test]
    fn rejects_unknown_scope() {
        let uris = vec!["https://app.example/cb".to_string()];
        let scopes = vec!["profile".to_string(), "admin".to_string()];
        assert_eq!(
            validate_input("App", "", "", &uris, &scopes).unwrap_err(),
            OAuthAppError::UnknownScope("admin".to_string())
        );
    }

    #[test]
    fn accepts_component_publish_scope() {
        let uris = vec!["https://app.example/cb".to_string()];
        let scopes = vec![SCOPE_COMPONENTS_PUBLISH.to_string()];
        let out = validate_input("App", "", "", &uris, &scopes).unwrap();
        assert_eq!(out.scopes, scopes);
    }

    #[test]
    fn dedupes_scopes_and_uris() {
        let uris = vec![
            "https://app.example/cb".to_string(),
            "https://app.example/cb".to_string(),
        ];
        let scopes = vec!["profile".to_string(), "profile".to_string()];
        let out = validate_input("App", "", "", &uris, &scopes).unwrap();
        assert_eq!(out.redirect_uris.len(), 1);
        assert_eq!(out.scopes.len(), 1);
    }

    #[test]
    fn https_redirect_is_accepted() {
        assert!(validate_redirect_uri("https://app.example/callback").is_ok());
    }

    #[test]
    fn http_loopback_is_accepted() {
        assert!(validate_redirect_uri("http://localhost:3000/cb").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1:8080/cb").is_ok());
    }

    #[test]
    fn http_non_loopback_is_rejected() {
        assert!(validate_redirect_uri("http://app.example/cb").is_err());
    }

    #[test]
    fn fragment_in_redirect_is_rejected() {
        assert!(validate_redirect_uri("https://app.example/cb#frag").is_err());
    }

    #[test]
    fn non_url_redirect_is_rejected() {
        assert!(validate_redirect_uri("not-a-url").is_err());
        assert!(validate_redirect_uri("ftp://app.example/cb").is_err());
    }

    #[test]
    fn generated_client_id_and_secret_are_distinct_and_prefixed() {
        let id1 = generate_client_id();
        let id2 = generate_client_id();
        assert_ne!(id1, id2);
        assert!(id1.starts_with("forest_oa_"));

        let s1 = generate_client_secret();
        let s2 = generate_client_secret();
        assert_ne!(s1, s2);
        assert!(s1.starts_with("forest_oas_"));
        assert_eq!(s1.len(), "forest_oas_".len() + 64);
    }

    #[test]
    fn id_token_is_hs256_verifiable_with_client_secret() {
        use hmac::{Hmac, Mac};
        use jwt::VerifyWithKey;
        use sha2::Sha256;
        use std::collections::BTreeMap;

        let secret = "forest_oas_supersecret";
        let token = mint_id_token(
            secret,
            "https://forest.example",
            "forest_oa_abc",
            "user-123",
            Some("alice"),
            Some("alice@example.com"),
            Some("n-abc"),
            1000,
            1000 + 3600,
        )
        .unwrap();

        let key: Hmac<Sha256> = Hmac::new_from_slice(secret.as_bytes()).unwrap();
        let claims: BTreeMap<String, String> = token.verify_with_key(&key).unwrap();
        assert_eq!(claims["iss"], "https://forest.example");
        assert_eq!(claims["sub"], "user-123");
        assert_eq!(claims["aud"], "forest_oa_abc");
        assert_eq!(claims["preferred_username"], "alice");
        assert_eq!(claims["email"], "alice@example.com");

        let wrong: Hmac<Sha256> = Hmac::new_from_slice(b"nope").unwrap();
        let bad: Result<BTreeMap<String, String>, _> = token.verify_with_key(&wrong);
        assert!(bad.is_err());
    }

    #[test]
    fn id_token_omits_claims_for_ungranted_scopes() {
        use hmac::{Hmac, Mac};
        use jwt::VerifyWithKey;
        use sha2::Sha256;
        use std::collections::BTreeMap;

        let token =
            mint_id_token("s", "iss", "cid", "user-1", None, None, None, 1000, 4600).unwrap();
        let key: Hmac<Sha256> = Hmac::new_from_slice(b"s").unwrap();
        let claims: BTreeMap<String, String> = token.verify_with_key(&key).unwrap();
        assert_eq!(claims["sub"], "user-1");
        assert!(!claims.contains_key("preferred_username"));
        assert!(!claims.contains_key("email"));
    }

    #[test]
    fn hash_is_stable_and_differs_per_secret() {
        let a = generate_client_secret();
        assert_eq!(hash_secret(&a), hash_secret(&a));
        assert_ne!(hash_secret(&a), hash_secret(&generate_client_secret()));
        assert_eq!(hash_secret(&a).len(), 32);
    }

    #[test]
    fn opaque_tokens_are_unique_prefixed_and_hashed() {
        let (raw1, hash1) = generate_opaque_token("forest_oat_");
        let (raw2, _) = generate_opaque_token("forest_oat_");
        assert_ne!(raw1, raw2);
        assert!(raw1.starts_with("forest_oat_"));
        assert_eq!(hash1.len(), 32);
        assert_eq!(hash1, hash_secret(&raw1));
    }

    #[test]
    fn constant_time_eq_matches_only_identical_slices() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn pkce_noop_without_challenge() {
        assert!(verify_pkce(None, None, None).is_ok());
        assert!(verify_pkce(None, Some("S256"), Some("anything")).is_ok());
    }

    #[test]
    fn pkce_plain_roundtrip() {
        let verifier = "the-verifier-value";
        assert!(verify_pkce(Some(verifier), Some("plain"), Some(verifier)).is_ok());
        assert_eq!(
            verify_pkce(Some(verifier), Some("plain"), Some("wrong")).unwrap_err(),
            OAuthAppError::PkceFailed
        );
    }

    #[test]
    fn pkce_s256_roundtrip() {
        use base64::Engine;
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        assert!(verify_pkce(Some(&challenge), Some("S256"), Some(verifier)).is_ok());
        assert_eq!(
            verify_pkce(Some(&challenge), Some("S256"), Some("wrong-verifier")).unwrap_err(),
            OAuthAppError::PkceFailed
        );
    }

    #[test]
    fn pkce_missing_verifier_when_challenge_present_fails() {
        assert_eq!(
            verify_pkce(Some("challenge"), Some("S256"), None).unwrap_err(),
            OAuthAppError::PkceFailed
        );
    }

    #[test]
    fn pkce_method_validation() {
        assert_eq!(normalize_pkce_method(None, None).unwrap(), None);
        assert_eq!(
            normalize_pkce_method(Some("c"), None).unwrap(),
            Some("plain".to_string())
        );
        assert_eq!(
            normalize_pkce_method(Some("c"), Some("S256")).unwrap(),
            Some("S256".to_string())
        );
        assert_eq!(
            normalize_pkce_method(Some("c"), Some("bogus")).unwrap_err(),
            OAuthAppError::InvalidCodeChallengeMethod
        );
    }

    #[test]
    fn grant_types_default_to_authorization_code_only() {
        assert_eq!(
            validate_grant_types(&[]).unwrap(),
            vec![GRANT_AUTHORIZATION_CODE.to_string()]
        );
    }

    #[test]
    fn grant_types_are_normalised_and_deduplicated() {
        let got = validate_grant_types(&[
            "  Client_Credentials ".to_string(),
            "client_credentials".to_string(),
            "authorization_code".to_string(),
        ])
        .unwrap();
        assert_eq!(
            got,
            vec![
                GRANT_CLIENT_CREDENTIALS.to_string(),
                GRANT_AUTHORIZATION_CODE.to_string()
            ]
        );
    }

    #[test]
    fn an_unknown_grant_type_is_refused_rather_than_ignored() {
        let err = validate_grant_types(&["implicit".to_string()]).unwrap_err();
        assert!(matches!(err, OAuthAppError::UnsupportedGrant(g) if g == "implicit"));
        assert!(
            validate_grant_types(&["authorization_code".to_string(), "password".to_string()])
                .is_err()
        );
    }

    fn app_with(grants: &[&str]) -> OAuthAppRow {
        OAuthAppRow {
            id: Uuid::nil(),
            organisation_id: Uuid::nil(),
            name: "app".into(),
            description: String::new(),
            homepage_url: String::new(),
            client_id: "cid".into(),
            client_secret_hash: vec![],
            redirect_uris: vec![],
            scopes: vec![],
            grant_types: grants.iter().map(|g| g.to_string()).collect(),
            public: false,
            created_by: Uuid::nil(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn an_app_may_hold_both_grants() {
        let both = validate_grant_types(&[
            GRANT_AUTHORIZATION_CODE.to_string(),
            GRANT_CLIENT_CREDENTIALS.to_string(),
        ])
        .unwrap();
        assert_eq!(both.len(), 2);

        let app = app_with(&[GRANT_AUTHORIZATION_CODE, GRANT_CLIENT_CREDENTIALS]);
        assert!(OAuthAppService::require_grant(&app, GRANT_AUTHORIZATION_CODE).is_ok());
        assert!(OAuthAppService::require_grant(&app, GRANT_CLIENT_CREDENTIALS).is_ok());
    }

    #[test]
    fn each_grant_is_refused_to_an_app_that_did_not_register_it() {
        let machine_only = app_with(&[GRANT_CLIENT_CREDENTIALS]);
        assert!(OAuthAppService::require_grant(&machine_only, GRANT_CLIENT_CREDENTIALS).is_ok());
        assert!(matches!(
            OAuthAppService::require_grant(&machine_only, GRANT_AUTHORIZATION_CODE),
            Err(OAuthAppError::UnsupportedGrant(g)) if g == GRANT_AUTHORIZATION_CODE
        ));

        let login_only = app_with(&[GRANT_AUTHORIZATION_CODE]);
        assert!(OAuthAppService::require_grant(&login_only, GRANT_AUTHORIZATION_CODE).is_ok());
        assert!(matches!(
            OAuthAppService::require_grant(&login_only, GRANT_CLIENT_CREDENTIALS),
            Err(OAuthAppError::UnsupportedGrant(g)) if g == GRANT_CLIENT_CREDENTIALS
        ));

        let none = app_with(&[]);
        assert!(OAuthAppService::require_grant(&none, GRANT_AUTHORIZATION_CODE).is_err());
        assert!(OAuthAppService::require_grant(&none, GRANT_CLIENT_CREDENTIALS).is_err());
    }

    #[test]
    fn machine_tokens_are_short_lived_relative_to_user_tokens() {
        assert!(CLIENT_TOKEN_TTL_SECONDS < ACCESS_TOKEN_TTL_SECONDS);
        assert!(CLIENT_TOKEN_TTL_SECONDS > 0);
    }

    #[test]
    fn a_registered_loopback_redirect_matches_any_port() {
        for registered in [
            "http://127.0.0.1/callback",
            "http://127.0.0.1:8080/callback",
        ] {
            assert!(redirect_uri_matches(
                registered,
                "http://127.0.0.1:53817/callback"
            ));
            assert!(redirect_uri_matches(
                registered,
                "http://127.0.0.1:1/callback"
            ));
            assert!(redirect_uri_matches(
                registered,
                "http://127.0.0.1/callback"
            ));
        }
        assert!(redirect_uri_matches(
            "http://localhost/callback",
            "http://localhost:49152/callback"
        ));
        assert!(redirect_uri_matches(
            "http://localhost:3000/cb?x=1",
            "http://localhost:4000/cb?x=1"
        ));
        assert!(redirect_uri_matches(
            "http://127.0.0.1",
            "http://127.0.0.1:9000/"
        ));
    }

    #[test]
    fn a_loopback_redirect_still_needs_the_same_host_path_and_query() {
        let registered = "http://127.0.0.1/callback";
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1:53817/other"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1:53817/callback/x"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://localhost:53817/callback"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.2:53817/callback"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1:53817/callback?a=b"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1.evil.example/callback"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1@evil.example/callback"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1:x/callback"
        ));
    }

    #[test]
    fn only_http_on_a_loopback_host_ignores_the_port() {
        assert!(!redirect_uri_matches(
            "https://127.0.0.1/callback",
            "https://127.0.0.1:53817/callback"
        ));
        assert!(!redirect_uri_matches(
            "http://127.0.0.1/callback",
            "https://127.0.0.1:53817/callback"
        ));
        assert!(!redirect_uri_matches(
            "https://app.example:8443/callback",
            "https://app.example:9443/callback"
        ));
        assert!(!redirect_uri_matches(
            "http://app.example/callback",
            "http://app.example:8080/callback"
        ));
        assert!(redirect_uri_matches(
            "https://app.example/callback",
            "https://app.example/callback"
        ));
    }

    #[test]
    fn a_public_client_may_only_sign_people_in_without_openid() {
        let code = vec![GRANT_AUTHORIZATION_CODE.to_string()];
        let device = vec![
            GRANT_AUTHORIZATION_CODE.to_string(),
            GRANT_DEVICE_CODE.to_string(),
        ];
        let both = vec![
            GRANT_AUTHORIZATION_CODE.to_string(),
            GRANT_CLIENT_CREDENTIALS.to_string(),
        ];
        let gateway = vec!["datagateway:apply".to_string(), "profile".to_string()];
        let oidc = vec!["openid".to_string(), "profile".to_string()];

        assert!(validate_public_client(&gateway, &code).is_ok());
        assert!(validate_public_client(&gateway, &device).is_ok());
        assert_eq!(
            validate_public_client(&gateway, &both).unwrap_err(),
            OAuthAppError::PublicClientGrant
        );
        assert_eq!(
            validate_public_client(&oidc, &code).unwrap_err(),
            OAuthAppError::PublicClientOpenid
        );
    }

    #[test]
    fn a_public_client_skips_the_secret_check_and_a_confidential_one_does_not() {
        let secret = generate_client_secret();
        let mut app = app_with(&[GRANT_AUTHORIZATION_CODE]);
        app.client_secret_hash = hash_secret(&secret);

        assert!(OAuthAppService::authenticate_client(&app, &secret).is_ok());
        assert_eq!(
            OAuthAppService::authenticate_client(&app, "").unwrap_err(),
            OAuthAppError::InvalidClientSecret
        );
        assert_eq!(
            OAuthAppService::authenticate_client(&app, "forest_oas_wrong").unwrap_err(),
            OAuthAppError::InvalidClientSecret
        );

        app.public = true;
        assert!(OAuthAppService::authenticate_client(&app, "").is_ok());
    }

    #[test]
    fn machine_and_user_tokens_are_distinguishable_by_prefix() {
        let (machine, _) = generate_opaque_token("forest_cat_");
        let (user, _) = generate_opaque_token("forest_oat_");
        assert!(machine.starts_with("forest_cat_"));
        assert!(user.starts_with("forest_oat_"));
        assert_ne!(machine, user);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use base64::Engine;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn constant_time_eq_matches_equality(a: Vec<u8>, b: Vec<u8>) {
            prop_assert_eq!(constant_time_eq(&a, &b), a == b);
        }

        #[test]
        fn constant_time_eq_reflexive(a: Vec<u8>) {
            prop_assert!(constant_time_eq(&a, &a));
        }

        #[test]
        fn pkce_s256_only_matching_verifier_passes(verifier in ".{1,128}") {
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(verifier.as_bytes()));
            prop_assert!(verify_pkce(Some(&challenge), Some("S256"), Some(&verifier)).is_ok());

            let wrong = format!("{verifier}x");
            prop_assert!(verify_pkce(Some(&challenge), Some("S256"), Some(&wrong)).is_err());
        }

        #[test]
        fn pkce_plain_matches_equality(challenge in ".{1,64}", verifier in ".{1,64}") {
            let result = verify_pkce(Some(&challenge), Some("plain"), Some(&verifier));
            prop_assert_eq!(result.is_ok(), challenge == verifier);
        }

        #[test]
        fn opaque_token_shape_is_invariant(prefix in "forest_[a-z]{2,4}_") {
            let (raw, hash) = generate_opaque_token(&prefix);
            prop_assert!(raw.starts_with(&prefix));
            prop_assert_eq!(raw.len(), prefix.len() + 64);
            prop_assert_eq!(hash.len(), 32);
            prop_assert_eq!(hash, hash_secret(&raw));
        }

        #[test]
        fn redirect_https_accepted_other_schemes_rejected(
            host in "[a-z]{1,20}\\.[a-z]{2,5}",
            path in "(/[a-z]{0,10}){0,3}",
            scheme in "(ftp|ws|javascript|file|data)",
        ) {
            let https = format!("https://{host}{path}");
            let other = format!("{scheme}://{host}{path}");
            prop_assert!(validate_redirect_uri(&https).is_ok());
            prop_assert!(validate_redirect_uri(&other).is_err());
        }
    }
}
