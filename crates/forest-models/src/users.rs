use chrono::{DateTime, Utc};
use uuid::Uuid;

// ─── Core user ───────────────────────────────────────────────────────

pub struct User {
    pub id: Uuid,
    pub username: String,
    pub emails: Vec<UserEmail>,
    pub oauth_connections: Vec<OAuthConnection>,
    pub mfa_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<User> for forest_grpc_interface::User {
    fn from(value: User) -> Self {
        Self {
            user_id: value.id.to_string(),
            username: value.username,
            emails: value.emails.into_iter().map(Into::into).collect(),
            oauth_connections: value.oauth_connections.into_iter().map(Into::into).collect(),
            mfa_enabled: value.mfa_enabled,
            created_at: Some(datetime_to_timestamp(value.created_at)),
            updated_at: Some(datetime_to_timestamp(value.updated_at)),
        }
    }
}

// ─── Email ───────────────────────────────────────────────────────────

pub struct UserEmail {
    pub email: String,
    pub verified: bool,
}

impl From<UserEmail> for forest_grpc_interface::UserEmail {
    fn from(value: UserEmail) -> Self {
        Self {
            email: value.email,
            verified: value.verified,
        }
    }
}

impl From<forest_grpc_interface::UserEmail> for UserEmail {
    fn from(value: forest_grpc_interface::UserEmail) -> Self {
        Self {
            email: value.email,
            verified: value.verified,
        }
    }
}

// ─── OAuth ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthProvider {
    Github,
    Google,
    Gitlab,
    Microsoft,
    MagicLink,
}

impl OAuthProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Google => "google",
            Self::Gitlab => "gitlab",
            Self::Microsoft => "microsoft",
            Self::MagicLink => "magic-link",
        }
    }
}

impl std::str::FromStr for OAuthProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(Self::Github),
            "google" => Ok(Self::Google),
            "gitlab" => Ok(Self::Gitlab),
            "microsoft" => Ok(Self::Microsoft),
            "magic-link" => Ok(Self::MagicLink),
            _ => Err(format!("unknown oauth provider: {s}")),
        }
    }
}

impl From<OAuthProvider> for forest_grpc_interface::OAuthProvider {
    fn from(value: OAuthProvider) -> Self {
        match value {
            OAuthProvider::Github => Self::OauthProviderGithub,
            OAuthProvider::Google => Self::OauthProviderGoogle,
            OAuthProvider::Gitlab => Self::OauthProviderGitlab,
            OAuthProvider::Microsoft => Self::OauthProviderMicrosoft,
            OAuthProvider::MagicLink => Self::OauthProviderMagicLink,
        }
    }
}

impl TryFrom<forest_grpc_interface::OAuthProvider> for OAuthProvider {
    type Error = String;

    fn try_from(value: forest_grpc_interface::OAuthProvider) -> Result<Self, Self::Error> {
        match value {
            forest_grpc_interface::OAuthProvider::OauthProviderGithub => Ok(Self::Github),
            forest_grpc_interface::OAuthProvider::OauthProviderGoogle => Ok(Self::Google),
            forest_grpc_interface::OAuthProvider::OauthProviderGitlab => Ok(Self::Gitlab),
            forest_grpc_interface::OAuthProvider::OauthProviderMicrosoft => Ok(Self::Microsoft),
            forest_grpc_interface::OAuthProvider::OauthProviderMagicLink => Ok(Self::MagicLink),
            forest_grpc_interface::OAuthProvider::OauthProviderUnspecified => {
                Err("unspecified oauth provider".into())
            }
        }
    }
}

pub struct OAuthConnection {
    pub provider: OAuthProvider,
    pub provider_user_id: String,
    pub provider_email: Option<String>,
    pub linked_at: DateTime<Utc>,
}

impl From<OAuthConnection> for forest_grpc_interface::OAuthConnection {
    fn from(value: OAuthConnection) -> Self {
        Self {
            provider: forest_grpc_interface::OAuthProvider::from(value.provider) as i32,
            provider_user_id: value.provider_user_id,
            provider_email: value.provider_email.unwrap_or_default(),
            linked_at: Some(datetime_to_timestamp(value.linked_at)),
        }
    }
}

// ─── Auth tokens ─────────────────────────────────────────────────────

pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: i64,
}

impl From<AuthTokens> for forest_grpc_interface::AuthTokens {
    fn from(value: AuthTokens) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_in_seconds: value.expires_in_seconds,
        }
    }
}

// ─── Session ─────────────────────────────────────────────────────────

pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ─── Personal access token ──────────────────────────────────────────

pub struct PersonalAccessToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ─── MFA ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaType {
    Totp,
}

impl MfaType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Totp => "totp",
        }
    }
}

impl std::str::FromStr for MfaType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "totp" => Ok(Self::Totp),
            _ => Err(format!("unknown mfa type: {s}")),
        }
    }
}

impl From<MfaType> for forest_grpc_interface::MfaType {
    fn from(value: MfaType) -> Self {
        match value {
            MfaType::Totp => Self::Totp,
        }
    }
}

impl TryFrom<forest_grpc_interface::MfaType> for MfaType {
    type Error = String;

    fn try_from(value: forest_grpc_interface::MfaType) -> Result<Self, Self::Error> {
        match value {
            forest_grpc_interface::MfaType::Totp => Ok(Self::Totp),
            forest_grpc_interface::MfaType::Unspecified => {
                Err("unspecified mfa type".into())
            }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}
