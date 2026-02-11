use anyhow::Context;
use forest_grpc_interface::login_request;

use crate::{
    grpc::GrpcClientState,
    state::State,
    user_state::{UserState, UserStateLoaderState, compute_refresh_after},
};

#[derive(clap::Parser)]
pub struct LoginCommand {
    /// Login with username (mutually exclusive with --email)
    #[arg(long, conflicts_with = "email")]
    username: Option<String>,

    /// Login with email (mutually exclusive with --username)
    #[arg(long, conflicts_with = "username")]
    email: Option<String>,
}

impl LoginCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let identifier = match (&self.username, &self.email) {
            (Some(username), _) => login_request::Identifier::Username(username.clone()),
            (_, Some(email)) => login_request::Identifier::Email(email.clone()),
            (None, None) => {
                // Interactive: ask what to use
                let choice =
                    inquire::Select::new("Login with:", vec!["Username", "Email"]).prompt()?;

                match choice {
                    "Username" => {
                        let username = inquire::Text::new("Username:").prompt()?;
                        login_request::Identifier::Username(username)
                    }
                    "Email" => {
                        let email = inquire::Text::new("Email:").prompt()?;
                        login_request::Identifier::Email(email)
                    }
                    _ => unreachable!(),
                }
            }
        };

        let password = inquire::Password::new("Password:")
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?;

        let resp = state
            .grpc_client()
            .login(identifier, &password)
            .await
            .context("failed to login")?;

        if let Some(user) = &resp.user {
            println!("Logged in as {} ({})", user.username, user.user_id);
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
