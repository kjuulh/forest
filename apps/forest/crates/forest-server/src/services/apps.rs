use uuid::Uuid;

// Return types shared between app_aggregate service and gRPC layer.

pub struct AppInfo {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub permissions: serde_json::Value,
    pub suspended: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct CreatedAppToken {
    pub token_id: Uuid,
    pub raw_token: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct AppTokenInfo {
    pub id: Uuid,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
