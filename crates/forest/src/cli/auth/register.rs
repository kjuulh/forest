use anyhow::Context;

use crate::{
    grpc::GrpcClientState,
    state::State,
    user_state::{UserState, UserStateLoaderState, compute_refresh_after},
};

#[derive(clap::Parser)]
pub struct RegisterCommand {
    /// Username for the new account
    #[arg(long)]
    username: Option<String>,

    /// Email for the new account
    #[arg(long)]
    email: Option<String>,
}

impl RegisterCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let username = match &self.username {
            Some(u) => u.clone(),
            None => inquire::Text::new("Username:").prompt()?,
        };

        let email = match &self.email {
            Some(e) => e.clone(),
            None => inquire::Text::new("Email:").prompt()?,
        };

        let password = inquire::Password::new("Password:")
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;

        let confirm = inquire::Password::new("Confirm password:")
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;

        if password != confirm {
            anyhow::bail!("passwords do not match");
        }

        let resp = state
            .grpc_client()
            .register(&username, &email, &password)
            .await
            .context("failed to register")?;

        if let Some(user) = &resp.user {
            println!("Registered as {} ({})", user.username, user.user_id);
        }

        let user = resp.user.unwrap();
        let tokens = resp.tokens.context("no tokens found, login is not valid")?;

        let now = chrono::Utc::now().timestamp();
        let refresh_after = compute_refresh_after(now, tokens.expires_in_seconds);

        state
            .user_state()
            .set_state(&UserState {
                user_id: user.user_id,
                username: user.username,
                emails: user.emails.into_iter().map(|e| e.email).collect(),
                access_token: tokens.access_token,
                refresh_access: tokens.refresh_token,
                refresh_after: Some(refresh_after),
            })
            .await?;

        Ok(())
    }
}
