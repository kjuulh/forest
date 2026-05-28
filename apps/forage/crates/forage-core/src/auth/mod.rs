pub mod linked;
pub mod magic_link;
mod validation;

pub use linked::{
    LinkOAuthInput, LinkedIdentity, LinkedProvider, ProviderDataExtras, link_input_from_github,
    link_input_from_oidc, linked_identity_from_forest, linked_identity_from_slack,
    merge_linked_identities,
};
pub use validation::{validate_email, validate_password, validate_username};

use serde::{Deserialize, Serialize};

/// Tokens returned by forest-server after login/register.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: i64,
}

/// Minimal user info from forest-server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub user_id: String,
    pub username: String,
    pub profile_picture_url: Option<String>,
    pub emails: Vec<UserEmail>,
    #[serde(default)]
    pub mfa_enabled: bool,
}

/// Public user profile (no emails).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub username: String,
    pub profile_picture_url: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserEmail {
    pub email: String,
    pub verified: bool,
}

/// A personal access token (metadata only, no raw key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalAccessToken {
    pub token_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: Option<String>,
    pub last_used: Option<String>,
    pub expires_at: Option<String>,
}

/// Result of creating a PAT - includes the raw key shown once.
#[derive(Debug, Clone)]
pub struct CreatedToken {
    pub token: PersonalAccessToken,
    pub raw_token: String,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("not authenticated")]
    NotAuthenticated,

    /// The caller is authenticated but the operation is not allowed for
    /// them (gRPC `PermissionDenied`). Distinct from `NotAuthenticated`
    /// so the UI can surface a 403 rather than redirecting to login.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("token expired")]
    TokenExpired,

    #[error("forest-server unavailable: {0}")]
    Unavailable(String),

    #[error("not found")]
    NotFound,

    /// Unlink would leave the account with no remaining sign-in method.
    /// Surfaced by `unlink_oauth_provider` when the user has no password
    /// and no other linked provider. Mapped from Forest's gRPC
    /// `FailedPrecondition("last_auth_method")`.
    #[error("cannot disconnect the only remaining sign-in method")]
    LastAuthMethod,

    #[error("{0}")]
    Other(String),
}

/// Result of a password login — may require MFA, or be blocked on
/// email verification.
#[derive(Debug, Clone)]
pub enum LoginResult {
    Success(AuthTokens),
    MfaRequired { mfa_session_token: String },
    /// The server requires the user to verify their email before logging
    /// in. Forage shows a "verify your email" page with a resend form.
    EmailNotVerified,
}

/// Result of a native register call. May either log the user in
/// immediately or request that the verification flow be driven before
/// any session is created.
#[derive(Debug, Clone)]
pub enum RegisterResult {
    Success(AuthTokens),
    /// Forest withheld tokens because email verification is required.
    /// The user row exists; forage now triggers the verification email.
    VerificationRequired,
}

/// Result of an `add_email` call.
#[derive(Debug, Clone)]
pub struct AddEmailResult {
    pub email: UserEmail,
    /// True when the server requires verification of the newly added
    /// email. Forage uses this to drive a per-email verification flow.
    pub email_verification_required: bool,
}

/// MFA setup info (QR code provisioning URI + base32 secret).
#[derive(Debug, Clone)]
pub struct MfaSetup {
    pub mfa_id: String,
    pub provisioning_uri: String,
    pub secret: String,
}

/// Result of an OAuth login via forest-server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthLoginResult {
    pub user: User,
    pub tokens: AuthTokens,
    pub is_new_user: bool,
}

/// Trait for communicating with forest-server's UsersService.
/// Object-safe via async_trait so we can use `Arc<dyn ForestAuth>`.
#[async_trait::async_trait]
pub trait ForestAuth: Send + Sync {
    async fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<RegisterResult, AuthError>;

    async fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<LoginResult, AuthError>;

    async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<AuthTokens, AuthError>;

    async fn logout(&self, refresh_token: &str) -> Result<(), AuthError>;

    async fn get_user(&self, access_token: &str) -> Result<User, AuthError>;

    async fn get_user_by_username(
        &self,
        access_token: &str,
        username: &str,
    ) -> Result<UserProfile, AuthError>;

    async fn get_user_by_email(
        &self,
        access_token: &str,
        email: &str,
    ) -> Result<UserProfile, AuthError>;

    async fn list_tokens(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<Vec<PersonalAccessToken>, AuthError>;

    async fn create_token(
        &self,
        access_token: &str,
        user_id: &str,
        name: &str,
    ) -> Result<CreatedToken, AuthError>;

    async fn delete_token(
        &self,
        access_token: &str,
        token_id: &str,
    ) -> Result<(), AuthError>;

    async fn update_username(
        &self,
        access_token: &str,
        user_id: &str,
        new_username: &str,
    ) -> Result<User, AuthError>;

    async fn update_profile_picture_url(
        &self,
        access_token: &str,
        user_id: &str,
        profile_picture_url: Option<&str>,
    ) -> Result<User, AuthError>;

    async fn change_password(
        &self,
        access_token: &str,
        user_id: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), AuthError>;

    async fn add_email(
        &self,
        access_token: &str,
        user_id: &str,
        email: &str,
    ) -> Result<AddEmailResult, AuthError>;

    /// Service-account-only confirmation of an email after a token
    /// redemption (called by the verify-email route).
    async fn confirm_email_verification(&self, email: &str) -> Result<(), AuthError>;

    async fn remove_email(
        &self,
        access_token: &str,
        user_id: &str,
        email: &str,
    ) -> Result<(), AuthError>;

    /// Log in or create a user via a pre-verified OAuth identity.
    /// The caller (Forage) has already exchanged the authorization code with
    /// the provider. This call requires service-account auth.
    async fn oauth_login(
        &self,
        provider: &str,
        provider_user_id: &str,
        provider_email: &str,
        provider_display_name: &str,
        picture_url: Option<&str>,
    ) -> Result<OAuthLoginResult, AuthError>;

    /// Complete login after MFA challenge.
    async fn verify_login_mfa(
        &self,
        mfa_session_token: &str,
        code: &str,
    ) -> Result<AuthTokens, AuthError>;

    /// Begin MFA setup (returns QR code provisioning URI).
    async fn setup_mfa(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<MfaSetup, AuthError>;

    /// Verify MFA setup with a TOTP code.
    async fn verify_mfa_setup(
        &self,
        access_token: &str,
        mfa_id: &str,
        code: &str,
    ) -> Result<(), AuthError>;

    /// Disable MFA (requires valid TOTP code).
    async fn disable_mfa(
        &self,
        access_token: &str,
        user_id: &str,
        code: &str,
    ) -> Result<(), AuthError>;

    /// List the OAuth identities linked to a user (GitHub, Google, etc.).
    /// Sourced from Forest's `identities` table via `GetUser`.
    /// Slack identities are NOT included — they live in Forage's
    /// `slack_user_links` and are merged in the route handler.
    async fn list_linked_identities(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<Vec<linked::LinkedIdentity>, AuthError>;

    /// Link an OAuth provider to an existing user.
    /// Maps Forest's `LinkOAuthProvider` RPC. The provider profile must
    /// already have been verified by the caller (OIDC exchange).
    ///
    /// Errors:
    /// - `AuthError::AlreadyExists("provider")` — this external account is
    ///   already linked to another Forage user.
    /// - `AuthError::AlreadyExists("user_provider")` — this user already
    ///   has a link for this provider; disconnect first.
    async fn link_oauth_provider(
        &self,
        access_token: &str,
        user_id: &str,
        input: &linked::LinkOAuthInput,
    ) -> Result<(), AuthError>;

    /// Unlink an OAuth provider from a user.
    /// Maps Forest's `UnlinkOAuthProvider` RPC. Idempotent at the API
    /// level (Forest returns success even if no row existed).
    async fn unlink_oauth_provider(
        &self,
        access_token: &str,
        user_id: &str,
        provider: linked::LinkedProvider,
    ) -> Result<(), AuthError>;

    /// Approve a forest CLI device-login grant on behalf of the
    /// browser-authenticated user. Service-account-only on the forest
    /// side. See apps/forest/TASKS/022-device-login.md.
    async fn approve_device_login(
        &self,
        user_code: &str,
        user_id: &str,
        approving_ip: &str,
        approving_user_agent: &str,
    ) -> Result<(), AuthError>;

    /// Deny a forest CLI device-login grant. Service-account-only.
    async fn deny_device_login(
        &self,
        user_code: &str,
        user_id: &str,
    ) -> Result<(), AuthError>;
}

/// Identity info obtained from an OIDC provider after exchanging the auth code.
#[derive(Debug, Clone)]
pub struct OidcIdentity {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub picture_url: Option<String>,
    /// Provider-native handle (e.g. GitHub `login`). None when the
    /// provider does not expose one separate from `name`.
    pub login: Option<String>,
}

/// Trait for exchanging an OAuth authorization code with a provider.
/// Abstracted for testability — tests can mock this without an HTTP server.
#[async_trait::async_trait]
pub trait OidcExchange: Send + Sync {
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<OidcIdentity, AuthError>;
}
