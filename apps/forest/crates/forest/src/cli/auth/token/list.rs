use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

use super::resolve_user_id;

#[derive(clap::Parser)]
pub struct ListTokensCommand {
    /// User ID to list tokens for. Defaults to the currently logged-in user.
    #[arg(long)]
    user_id: Option<String>,
}

impl ListTokensCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let user_id = resolve_user_id(state, self.user_id.as_deref()).await?;

        let tokens = state
            .grpc_client()
            .list_personal_access_tokens(&user_id)
            .await
            .context("failed to list tokens")?;

        if tokens.is_empty() {
            eprintln!("No personal access tokens");
            return Ok(());
        }

        for token in tokens {
            println!("{}\t{}", token.token_id, token.name);

            if !token.scopes.is_empty() {
                println!("  scopes: {}", token.scopes.join(", "));
            }

            if let Some(ts) = token.expires_at {
                let expires = chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "unknown".into());
                println!("  expires: {}", expires);
            }

            if let Some(ts) = token.last_used {
                let used = chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "unknown".into());
                println!("  last used: {}", used);
            }
        }

        Ok(())
    }
}
