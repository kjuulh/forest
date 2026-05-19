use uuid::Uuid;

/// Represents who performed an action — either a human user, an app, or a
/// service account (long-lived infrastructure key).
#[derive(Debug, Clone)]
pub enum Actor {
    User { user_id: Uuid },
    App { app_id: Uuid, organisation_id: Uuid },
    ServiceAccount { service_account_id: Uuid },
}

impl Actor {
    pub fn actor_id(&self) -> Uuid {
        match self {
            Actor::User { user_id } => *user_id,
            Actor::App { app_id, .. } => *app_id,
            Actor::ServiceAccount { service_account_id } => *service_account_id,
        }
    }

    pub fn actor_type(&self) -> &'static str {
        match self {
            Actor::User { .. } => "user",
            Actor::App { .. } => "app",
            Actor::ServiceAccount { .. } => "service_account",
        }
    }
}
