use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateTokenCommand {
    /// User ID to create the token for
    #[arg(long)]
    user_id: String,

    /// Name for the token
    #[arg(long)]
    name: Option<String>,

    /// Comma-separated scopes
    #[arg(long, value_delimiter = ',')]
    scopes: Vec<String>,

    /// Expiry in seconds (0 = no expiry)
    #[arg(long, default_value = "0")]
    expires_in: i64,
}

impl CreateTokenCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let name = match &self.name {
            Some(n) => n.clone(),
            None => inquire::Text::new("Token name:").prompt()?,
        };

        let resp = state
            .grpc_client()
            .create_personal_access_token(&self.user_id, &name, self.scopes.clone(), self.expires_in)
            .await
            .context("failed to create token")?;

        if let Some(token) = resp.token {
            println!("Token ID:   {}", token.token_id);
            println!("Name:       {}", token.name);
            if !token.scopes.is_empty() {
                println!("Scopes:     {}", token.scopes.join(", "));
            }
        }

        if !resp.raw_token.is_empty() {
            println!();
            println!("Token: {}", resp.raw_token);
            println!();
            println!("Make sure to copy this token now. You won't be able to see it again.");
        }

        Ok(())
    }
}
