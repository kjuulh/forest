pub mod linked;
pub mod magic_link;
pub mod oauth_state;
mod validation;

pub use linked::{
    LinkOAuthInput, LinkedIdentity, LinkedProvider, ProviderDataExtras, link_input_from_github,
    link_input_from_oidc, linked_identity_from_forest, linked_identity_from_slack,
    merge_linked_identities,
};
pub use validation::{validate_email, validate_password, validate_username};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub user_id: String,
    pub username: String,
    pub profile_picture_url: Option<String>,
    pub emails: Vec<UserEmail>,
    #[serde(default)]
    pub mfa_enabled: bool,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalAccessToken {
    pub token_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: Option<String>,
    pub last_used: Option<String>,
    pub expires_at: Option<String>,
}

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

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("token expired")]
    TokenExpired,

    #[error("forest-server unavailable: {0}")]
    Unavailable(String),

    #[error("not found")]
    NotFound,

    #[error("cannot disconnect the only remaining sign-in method")]
    LastAuthMethod,

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub enum LoginResult {
    Success(AuthTokens),
    MfaRequired { mfa_session_token: String },
    EmailNotVerified,
}

#[derive(Debug, Clone)]
pub enum RegisterResult {
    Success(AuthTokens),
    VerificationRequired,
}

#[derive(Debug, Clone)]
pub struct AddEmailResult {
    pub email: UserEmail,
    pub email_verification_required: bool,
}

#[derive(Debug, Clone)]
pub struct MfaSetup {
    pub mfa_id: String,
    pub provisioning_uri: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthLoginResult {
    pub user: User,
    pub tokens: AuthTokens,
    pub is_new_user: bool,
}

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

    async fn confirm_email_verification(&self, email: &str) -> Result<(), AuthError>;

    async fn remove_email(
        &self,
        access_token: &str,
        user_id: &str,
        email: &str,
    ) -> Result<(), AuthError>;

    /// Requires service-account auth and an identity the caller already verified.
    async fn oauth_login(
        &self,
        provider: &str,
        provider_user_id: &str,
        provider_email: &str,
        provider_display_name: &str,
        picture_url: Option<&str>,
    ) -> Result<OAuthLoginResult, AuthError>;

    async fn verify_login_mfa(
        &self,
        mfa_session_token: &str,
        code: &str,
    ) -> Result<AuthTokens, AuthError>;

    async fn setup_mfa(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<MfaSetup, AuthError>;

    async fn verify_mfa_setup(
        &self,
        access_token: &str,
        mfa_id: &str,
        code: &str,
    ) -> Result<(), AuthError>;

    async fn disable_mfa(
        &self,
        access_token: &str,
        user_id: &str,
        code: &str,
    ) -> Result<(), AuthError>;

    /// Excludes Slack identities, which live in Forage's `slack_user_links`.
    async fn list_linked_identities(
        &self,
        access_token: &str,
        user_id: &str,
    ) -> Result<Vec<linked::LinkedIdentity>, AuthError>;

    /// `AlreadyExists("provider")`: taken by another user; `"user_provider"`: already linked here.
    async fn link_oauth_provider(
        &self,
        access_token: &str,
        user_id: &str,
        input: &linked::LinkOAuthInput,
    ) -> Result<(), AuthError>;

    /// Idempotent: succeeds even when no link existed.
    async fn unlink_oauth_provider(
        &self,
        access_token: &str,
        user_id: &str,
        provider: linked::LinkedProvider,
    ) -> Result<(), AuthError>;

    async fn approve_device_login(
        &self,
        user_code: &str,
        user_id: &str,
        approving_ip: &str,
        approving_user_agent: &str,
    ) -> Result<(), AuthError>;

    async fn deny_device_login(
        &self,
        user_code: &str,
        user_id: &str,
    ) -> Result<(), AuthError>;
}

#[derive(Debug, Clone)]
pub struct OidcIdentity {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub picture_url: Option<String>,
    pub login: Option<String>,
}

#[async_trait::async_trait]
pub trait OidcExchange: Send + Sync {
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<OidcIdentity, AuthError>;
}
