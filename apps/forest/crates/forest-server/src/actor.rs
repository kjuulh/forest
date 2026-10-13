use uuid::Uuid;

/// Represents who performed an action — either a human user, an app, or a
/// service account (long-lived infrastructure key).
#[derive(Debug, Clone)]
pub enum Actor {
    User {
        user_id: Uuid,
    },
    App {
        app_id: Uuid,
        organisation_id: Uuid,
        /// `None` for a legacy app token, which acts org-wide as an app
        /// always has. `Some` for an OAuth client-credentials token, which
        /// may act only within the scopes it was granted: the auth layer
        /// refuses any method those scopes do not cover
        /// (`grpc::auth_layer::scoped_token_permits`).
        scopes: Option<Vec<String>>,
    },
    ServiceAccount {
        service_account_id: Uuid,
    },
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
