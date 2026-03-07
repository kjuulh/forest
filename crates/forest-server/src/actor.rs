use uuid::Uuid;

/// Represents who performed an action — either a human user or an app.
#[derive(Debug, Clone)]
pub enum Actor {
    User { user_id: Uuid },
    App { app_id: Uuid, organisation_id: Uuid },
}

impl Actor {
    pub fn actor_id(&self) -> Uuid {
        match self {
            Actor::User { user_id } => *user_id,
            Actor::App { app_id, .. } => *app_id,
        }
    }

    pub fn actor_type(&self) -> &'static str {
        match self {
            Actor::User { .. } => "user",
            Actor::App { .. } => "app",
        }
    }
}
