use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

use super::resolve_user_id;

#[derive(clap::Parser)]
pub struct CreateTokenCommand {
    /// User ID to create the token for. Defaults to the currently logged-in user.
    #[arg(long)]
    user_id: Option<String>,

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
        let user_id = resolve_user_id(state, self.user_id.as_deref()).await?;

        let name = match &self.name {
            Some(n) => n.clone(),
            None => inquire::Text::new("Token name:").prompt()?,
        };

        let resp = state
            .grpc_client()
            .create_personal_access_token(&user_id, &name, self.scopes.clone(), self.expires_in)
            .await
            .context("failed to create token")?;

        // Metadata + warnings to stderr so `forest auth token create > token.txt`
        // captures only the raw token (data).
        if let Some(token) = resp.token {
            eprintln!("Token ID:   {}", token.token_id);
            eprintln!("Name:       {}", token.name);
            if !token.scopes.is_empty() {
                eprintln!("Scopes:     {}", token.scopes.join(", "));
            }
        }

        if !resp.raw_token.is_empty() {
            eprintln!();
            println!("{}", resp.raw_token);
            eprintln!();
            eprintln!("Make sure to copy this token now. You won't be able to see it again.");
        }

        Ok(())
    }
}
