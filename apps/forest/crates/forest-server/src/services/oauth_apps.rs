use sha2::Digest;
use uuid::Uuid;

use crate::{
    State,
    repositories::oauth_apps::{OAuthAppRepository, OAuthAppRepositoryState, OAuthAppRow},
};

/// The fixed scope catalog an OAuth app may request (MVP).
///
/// - `openid`:  OIDC marker — issues an `id_token` on the code exchange
/// - `profile`: sub (user_id), username, profile_picture_url
/// - `email`:   primary + all verified emails
pub const ALLOWED_SCOPES: &[&str] = &["openid", "profile", "email", "directory:read"];

/// Scope for reading the organisation directory — resolving a person from
/// an external identity to their linked accounts. Only ever granted to
/// machine clients; there is nothing here a browser login needs.
pub const SCOPE_DIRECTORY_READ: &str = "directory:read";

/// Validation failures for OAuth-app management. The gRPC layer maps these
/// to `invalid_argument` with the message surfaced to the caller.
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

    // ── Authorization-server flow errors ──
    /// No app exists for the presented client_id.
    #[error("unknown client")]
    UnknownClient,
    /// client_secret did not match.
    #[error("invalid client credentials")]
    InvalidClientSecret,
    /// The app is not registered for the grant it tried to use.
    #[error("client is not authorised for the {0} grant")]
    UnsupportedGrant(String),
    /// A machine app asked for a scope it was never registered with.
    #[error("scope not registered for this client: {0}")]
    ScopeNotGranted(String),
    /// redirect_uri is not in the app's allowlist.
    #[error("redirect_uri not registered for this client")]
    RedirectUriNotAllowed,
    /// The authorization code is missing, expired, already used, or bound to a
    /// different client / redirect_uri.
    #[error("invalid or expired authorization code")]
    InvalidGrant,
    /// PKCE verification failed.
    #[error("PKCE verification failed")]
    PkceFailed,
    /// Unsupported code_challenge_method.
    #[error("unsupported code_challenge_method")]
    InvalidCodeChallengeMethod,
}

/// Public view of an OAuth app — never carries the client_secret.
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
    /// Which OAuth grants this app may use.
    pub grant_types: Vec<String>,
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
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// An app paired with its freshly-minted raw client_secret. Returned only
/// from create / rotate; the secret is shown to the org once and never again.
#[derive(Debug, Clone)]
pub struct CreatedOAuthApp {
    pub app: OAuthApp,
    pub client_secret: String,
}

/// Authorization-code lifetime (single-use, short-lived per RFC 6749 §4.1.2).
pub const AUTH_CODE_TTL_SECONDS: i64 = 60;
/// Access-token lifetime.
pub const ACCESS_TOKEN_TTL_SECONDS: i64 = 8 * 3600;
/// Machine-token lifetime. Much shorter than a user's access token: a
/// client-credentials holder can re-mint at will with the secret it
/// already has, so a long life buys nothing and costs blast radius.
pub const CLIENT_TOKEN_TTL_SECONDS: i64 = 3600;

/// `grant_types` vocabulary.
pub const GRANT_AUTHORIZATION_CODE: &str = "authorization_code";
pub const GRANT_CLIENT_CREDENTIALS: &str = "client_credentials";

/// Grants an app may be registered for.
pub const SUPPORTED_GRANT_TYPES: &[&str] = &[GRANT_AUTHORIZATION_CODE, GRANT_CLIENT_CREDENTIALS];
/// Refresh-token lifetime.
pub const REFRESH_TOKEN_TTL_SECONDS: i64 = 90 * 24 * 3600;

/// A minted machine token. No refresh token and no id_token — see
/// `client_credentials_token` for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedClientToken {
    pub access_token: String,
    pub expires_in_seconds: i64,
    pub scopes: Vec<String>,
}

/// Who a machine token belongs to, as seen by a resource server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPrincipal {
    pub app_id: Uuid,
    pub organisation_id: Uuid,
    pub scopes: Vec<String>,
}

/// Opaque tokens minted on a successful code exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: i64,
    pub scopes: Vec<String>,
    /// OIDC id_token (HS256 JWT signed with the client_secret), present only
    /// when the `openid` scope was granted.
    pub id_token: Option<String>,
}

/// A user's authorization of an app (one per app), for the account UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrant {
    pub app_id: uuid::Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub authorized_at: chrono::DateTime<chrono::Utc>,
}

/// User claims resolved from an access token, gated by granted scopes. `sub`
/// is always present; the rest are `None`/empty unless the scope was granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Userinfo {
    pub sub: String,
    pub username: Option<String>,
    pub profile_picture_url: Option<String>,
    pub email: Option<String>,
    pub emails: Vec<String>,
    pub scopes: Vec<String>,
}

/// Validated, normalized inputs for create/update.
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
}

impl OAuthAppService {
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
    ) -> anyhow::Result<CreatedOAuthApp> {
        let input = validate_input(name, description, homepage_url, redirect_uris, scopes)?;
        let grant_types = validate_grant_types(grant_types)?;

        let app_id = Uuid::now_v7();
        let client_id = generate_client_id();
        let client_secret = generate_client_secret();
        let secret_hash = hash_secret(&client_secret);

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

    // ── Authorization server ──────────────────────────────────────────

    /// Public app metadata for the consent screen, by client_id.
    pub async fn lookup_client(&self, client_id: &str) -> anyhow::Result<Option<OAuthApp>> {
        let row = self
            .repo
            .get_oauth_app_by_client_id(self.repo.pool(), client_id)
            .await?;
        Ok(row.map(OAuthApp::from))
    }

    /// Mint a single-use authorization code, after the user has consented.
    /// Re-validates client, redirect_uri (exact match), and scope subset.
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

        // Checked before anything else user-visible happens: a machine-only
        // app should never reach a consent screen at all.
        Self::require_grant(&app, GRANT_AUTHORIZATION_CODE)?;

        if !app.redirect_uris.iter().any(|u| u == redirect_uri) {
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

        // The user just consented to these scopes — remember it so a later
        // authorization for the same (or narrower) scopes can skip the screen.
        self.repo
            .upsert_consent(self.repo.pool(), app.id, user_id, scopes)
            .await?;

        Ok((code, AUTH_CODE_TTL_SECONDS))
    }

    /// Scopes the user has previously consented to for a client (empty when
    /// none). Used by Forage to decide whether to skip the consent screen.
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

    /// Exchange an authorization code for access + refresh tokens. Verifies
    /// client credentials, single-use consumption, redirect_uri binding, and
    /// PKCE (when a challenge was stored).
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

        if !constant_time_eq(&hash_secret(client_secret), &row.client_secret_hash) {
            return Err(OAuthAppError::InvalidClientSecret.into());
        }

        // Re-checked here rather than trusted from issue time: a code
        // outlives the check that minted it, and the grant may have been
        // taken away in between.
        Self::require_grant(&row, GRANT_AUTHORIZATION_CODE)?;

        let code_hash = hash_secret(code);
        let consumed = self
            .repo
            .consume_authorization_code(self.repo.pool(), &code_hash)
            .await?
            .ok_or(OAuthAppError::InvalidGrant)?;

        // The code must belong to this client and the same redirect_uri.
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
                issuer,
                client_id,
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

    /// Build an OIDC `id_token` when the `openid` scope was granted, loading
    /// the profile/email claims the granted scopes permit. `None` otherwise.
    #[allow(clippy::too_many_arguments)]
    async fn maybe_id_token(
        &self,
        issuer: &str,
        client_id: &str,
        client_secret: &str,
        user_id: Uuid,
        scopes: &[String],
        nonce: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        if !scopes.iter().any(|s| s == "openid") {
            return Ok(None);
        }
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

    /// Refuse an app a grant it was never registered for.
    ///
    /// Both directions matter. An app is free to hold *both* grants —
    /// acting for a user and acting as itself, the way a GitHub App does
    /// — but it should only be able to use what it declared, so a
    /// machine-only credential can't quietly run a login flow and a
    /// login-only app can't mint machine tokens.
    fn require_grant(app: &OAuthAppRow, grant: &str) -> Result<(), OAuthAppError> {
        if app.grant_types.iter().any(|g| g == grant) {
            return Ok(());
        }
        Err(OAuthAppError::UnsupportedGrant(grant.to_string()))
    }

    /// The `client_credentials` grant: an app authenticating as *itself*,
    /// with no user in the loop.
    ///
    /// Deliberately narrower than the code flow. There is no refresh
    /// token (the client re-mints with the secret it already holds), no
    /// id_token (there is no subject to describe), and no consent (an
    /// app cannot consent on a user's behalf when no user is involved).
    ///
    /// Requested scopes must be a subset of the app's registered scopes;
    /// asking for more is an error rather than a silent downgrade, so a
    /// misconfigured client fails loudly instead of quietly getting less
    /// access than it thinks it has.
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

        // Empty request means "everything this app is registered for" —
        // the conventional reading, and it keeps simple clients simple.
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

    /// Resolve a machine token to the app behind it. Resource servers
    /// call this to authorise a request.
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
        }))
    }

    /// Exchange a refresh token for a fresh access + refresh token, rotating
    /// the refresh token. Detects reuse of an already-rotated refresh token and
    /// revokes the whole (user, app) grant family as a defence (RFC 6749 §10.4).
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
        if !constant_time_eq(&hash_secret(client_secret), &app.client_secret_hash) {
            return Err(OAuthAppError::InvalidClientSecret.into());
        }
        // A refresh token can only have come from the code flow, so
        // withdrawing that grant should stop the whole family working —
        // not just stop new logins while old sessions roll on forever.
        Self::require_grant(&app, GRANT_AUTHORIZATION_CODE)?;

        let refresh_hash = hash_secret(refresh_token);

        // Atomically consume the refresh token (single-use even under
        // concurrent refreshes — only one UPDATE can match `revoked_at IS NULL`).
        let consumed = self
            .repo
            .consume_refresh_token(self.repo.pool(), &refresh_hash, app.id)
            .await?;

        let consumed = match consumed {
            Some(row) => row,
            None => {
                // Consume failed. If the token exists but is already revoked,
                // this is reuse of a rotated token → revoke the whole family
                // (RFC 6749 §10.4). Otherwise it's simply invalid/expired.
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

        // Per OIDC, `nonce` belongs to the original authentication request and
        // is not carried into id_tokens minted on refresh.
        let id_token = self
            .maybe_id_token(
                issuer,
                client_id,
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

    /// List the apps a user has authorized, one entry per app, unioning scopes
    /// across that app's live tokens and reporting the earliest authorization.
    pub async fn list_grants(&self, user_id: Uuid) -> anyhow::Result<Vec<OAuthGrant>> {
        let rows = self
            .repo
            .list_live_grant_rows(self.repo.pool(), user_id)
            .await?;

        // Rows are ordered by created_at ASC, so the first time we see an app
        // is its earliest grant. Preserve that order in the output.
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

    /// Revoke a user's grant for an app — drops all of its issued tokens and
    /// forgets the remembered consent (so the next authorization re-prompts).
    /// Returns the number of live tokens revoked.
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

    /// Resolve an access token to user claims, gated by the token's scopes.
    /// Returns `InvalidGrant` when the token is unknown, expired, or revoked.
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

// ─── Pure helpers (unit-tested without a database) ────────────────────

/// Validate and normalize create/update inputs.
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

/// A redirect URI must be an absolute http(s) URL with no fragment. https is
/// required, except for loopback (localhost / 127.0.0.1) which may use http
/// for local development.
/// Normalise and check an app's requested grant types.
///
/// An empty list means `authorization_code`, matching both the column
/// default and every app that existed before this grant did — so an app
/// only becomes machine-capable by asking for it.
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

/// Public client identifier. Random and unguessable but not secret.
pub fn generate_client_id() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes[..]);
    format!("forest_oa_{}", hex::encode(bytes))
}

/// Raw client secret, shown to the org exactly once.
pub fn generate_client_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes[..]);
    format!("forest_oas_{}", hex::encode(bytes))
}

/// SHA-256 of the raw secret; only this is persisted.
pub fn hash_secret(secret: &str) -> Vec<u8> {
    sha2::Sha256::digest(secret.as_bytes()).to_vec()
}

/// Generate an opaque, unguessable token (`<prefix><64 hex chars>`) and its
/// SHA-256 hash. Only the hash is stored; the raw value is returned once.
pub fn generate_opaque_token(prefix: &str) -> (String, Vec<u8>) {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes[..]);
    let raw = format!("{prefix}{}", hex::encode(bytes));
    let hash = sha2::Sha256::digest(raw.as_bytes()).to_vec();
    (raw, hash)
}

/// Treat an empty string as absent (proto3 has no optional-string nuance here).
fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

/// Validate the PKCE method against the presence of a challenge. Returns the
/// effective method to persist (defaults to "plain" when a challenge is given
/// without an explicit method, per RFC 7636 §4.3).
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

/// Verify a PKCE `code_verifier` against the stored challenge. A no-op when no
/// challenge was registered for the code.
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

/// Mint an OIDC `id_token`: a JWT signed with HS256 using the client_secret as
/// the key (RFC 7519 + OIDC Core §3.1.3.7). Confidential clients verify it with
/// their own secret, so no asymmetric key management / JWKS is needed. `sub`,
/// `iss`, `aud`, `iat`, `exp` are always set; `preferred_username` / `email`
/// only when the caller passes them (scope-gated upstream).
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

/// Length-independent constant-time byte comparison (avoids leaking where two
/// secrets/hashes first differ).
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

// ─── State trait ─────────────────────────────────────────────────────

pub trait OAuthAppServiceState {
    fn oauth_app_service(&self) -> OAuthAppService;
}

impl OAuthAppServiceState for State {
    fn oauth_app_service(&self) -> OAuthAppService {
        OAuthAppService {
            repo: self.oauth_app_repository(),
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
        // 32 bytes hex = 64 chars + prefix
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

        // A client with the client_secret can verify + read the claims.
        let key: Hmac<Sha256> = Hmac::new_from_slice(secret.as_bytes()).unwrap();
        let claims: BTreeMap<String, String> = token.verify_with_key(&key).unwrap();
        assert_eq!(claims["iss"], "https://forest.example");
        assert_eq!(claims["sub"], "user-123");
        assert_eq!(claims["aud"], "forest_oa_abc");
        assert_eq!(claims["preferred_username"], "alice");
        assert_eq!(claims["email"], "alice@example.com");

        // The wrong secret fails verification.
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

        // openid only — no profile/email.
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
        assert_eq!(hash1, hash_secret(&raw1)); // hash == sha256(raw)
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

    // ── client_credentials grant ──────────────────────────────────────

    #[test]
    fn grant_types_default_to_authorization_code_only() {
        // An app that says nothing is a login app. This is what stops
        // every pre-existing app from silently gaining machine access
        // when the column was added.
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
        // Silently dropping it would register an app that appears to
        // support a grant it does not.
        let err = validate_grant_types(&["implicit".to_string()]).unwrap_err();
        assert!(matches!(err, OAuthAppError::UnsupportedGrant(g) if g == "implicit"));
        // And one bad entry fails the whole list.
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
            created_by: Uuid::nil(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    /// The GitHub-App shape: one credential that both acts for a user
    /// and acts as itself.
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

    /// `grant_types` has to bite in *both* directions, or it is
    /// decoration. A machine-only credential must not be able to run a
    /// login flow.
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

        // An app with nothing registered can do nothing — it cannot fall
        // back to "whatever was asked for".
        let none = app_with(&[]);
        assert!(OAuthAppService::require_grant(&none, GRANT_AUTHORIZATION_CODE).is_err());
        assert!(OAuthAppService::require_grant(&none, GRANT_CLIENT_CREDENTIALS).is_err());
    }

    #[test]
    fn machine_tokens_are_short_lived_relative_to_user_tokens() {
        // A client-credentials holder can re-mint at will with the
        // secret it already has, so a long life buys nothing and only
        // widens the blast radius of a leak.
        assert!(CLIENT_TOKEN_TTL_SECONDS < ACCESS_TOKEN_TTL_SECONDS);
        assert!(CLIENT_TOKEN_TTL_SECONDS > 0);
    }

    #[test]
    fn machine_and_user_tokens_are_distinguishable_by_prefix() {
        // The two live in separate tables and must never be accepted
        // interchangeably; distinct prefixes make a mix-up obvious in
        // logs and in a bug report.
        let (machine, _) = generate_opaque_token("forest_cat_");
        let (user, _) = generate_opaque_token("forest_oat_");
        assert!(machine.starts_with("forest_cat_"));
        assert!(user.starts_with("forest_oat_"));
        assert_ne!(machine, user);
    }
}

/// Property-based tests for the security-critical pure helpers (VSDD Phase 5).
#[cfg(test)]
mod proptests {
    use super::*;
    use base64::Engine;
    use proptest::prelude::*;

    proptest! {
        /// constant_time_eq agrees with `==` for arbitrary byte strings.
        #[test]
        fn constant_time_eq_matches_equality(a: Vec<u8>, b: Vec<u8>) {
            prop_assert_eq!(constant_time_eq(&a, &b), a == b);
        }

        /// A slice is always constant-time-equal to itself.
        #[test]
        fn constant_time_eq_reflexive(a: Vec<u8>) {
            prop_assert!(constant_time_eq(&a, &a));
        }

        /// PKCE S256: the matching verifier always validates; a different
        /// verifier never does. (Property: only the preimage of the challenge
        /// passes.)
        #[test]
        fn pkce_s256_only_matching_verifier_passes(verifier in ".{1,128}") {
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(sha2::Sha256::digest(verifier.as_bytes()));
            prop_assert!(verify_pkce(Some(&challenge), Some("S256"), Some(&verifier)).is_ok());

            let wrong = format!("{verifier}x");
            prop_assert!(verify_pkce(Some(&challenge), Some("S256"), Some(&wrong)).is_err());
        }

        /// PKCE plain: passes iff verifier equals challenge.
        #[test]
        fn pkce_plain_matches_equality(challenge in ".{1,64}", verifier in ".{1,64}") {
            let result = verify_pkce(Some(&challenge), Some("plain"), Some(&verifier));
            prop_assert_eq!(result.is_ok(), challenge == verifier);
        }

        /// Generated tokens always carry their prefix and a 32-byte (64-hex)
        /// random body, and the stored hash is sha256(raw).
        #[test]
        fn opaque_token_shape_is_invariant(prefix in "forest_[a-z]{2,4}_") {
            let (raw, hash) = generate_opaque_token(&prefix);
            prop_assert!(raw.starts_with(&prefix));
            prop_assert_eq!(raw.len(), prefix.len() + 64);
            prop_assert_eq!(hash.len(), 32);
            prop_assert_eq!(hash, hash_secret(&raw));
        }

        /// Any https URL with a host and no fragment is an accepted redirect;
        /// non-http(s) schemes are always rejected.
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
