use anyhow::Context;
use forest_grpc_interface::get_user_request;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct StatusCommand {
    /// Show status for a specific user by ID
    #[arg(long)]
    user_id: Option<String>,

    /// Show status for a specific user by username
    #[arg(long)]
    username: Option<String>,
}

impl StatusCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let identifier = match (&self.user_id, &self.username) {
            (Some(id), _) => get_user_request::Identifier::UserId(id.clone()),
            (_, Some(name)) => get_user_request::Identifier::Username(name.clone()),
            (None, None) => {
                // TODO: read user_id from stored auth token
                anyhow::bail!(
                    "provide --user-id or --username (automatic detection requires stored session)"
                );
            }
        };

        let user = state
            .grpc_client()
            .get_user(identifier)
            .await
            .context("failed to get user")?
            .ok_or_else(|| anyhow::anyhow!("user not found"))?;

        println!("User ID:    {}", user.user_id);
        println!("Username:   {}", user.username);

        if !user.emails.is_empty() {
            println!("Emails:");
            for email in &user.emails {
                let verified = if email.verified { " (verified)" } else { "" };
                println!("  {}{}", email.email, verified);
            }
        }

        if !user.oauth_connections.is_empty() {
            println!("OAuth connections:");
            for conn in &user.oauth_connections {
                let provider = forest_grpc_interface::OAuthProvider::try_from(conn.provider)
                    .map(|p| format!("{:?}", p))
                    .unwrap_or_else(|_| "unknown".into());
                println!("  {} ({})", provider, conn.provider_user_id);
            }
        }

        println!(
            "MFA:        {}",
            if user.mfa_enabled {
                "enabled"
            } else {
                "disabled"
            }
        );

        if let Some(ts) = user.created_at {
            println!(
                "Created:    {}",
                chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "unknown".into())
            );
        }

        Ok(())
    }
}
