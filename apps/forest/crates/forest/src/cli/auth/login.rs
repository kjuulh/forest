use anyhow::Context;
use forest_grpc_interface::login_request;

use crate::{
    grpc::GrpcClientState,
    state::State,
    user_state::{UserState, UserStateLoaderState, compute_refresh_after},
};

use super::login_web;

#[derive(clap::Parser)]
pub struct LoginCommand {
    /// Open a browser and approve the login there (RFC 8628 device
    /// authorization grant). Default when invoked without a mode flag
    /// in non-TTY contexts. Mutually exclusive with --password.
    #[arg(long, conflicts_with_all = ["password_flag", "username", "email"])]
    web: bool,

    /// Use the legacy username/email + password flow. Required for
    /// scripts that pipe passwords from stdin or set FOREST_PASSWORD.
    /// Mutually exclusive with --web.
    #[arg(long = "password")]
    password_flag: bool,

    /// Login with username. Only valid with --password.
    #[arg(long, conflicts_with = "email", visible_alias = "user")]
    username: Option<String>,

    /// Login with email. Only valid with --password.
    #[arg(long, conflicts_with = "username")]
    email: Option<String>,
}

impl LoginCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // Username/email implies password mode.
        let want_password = self.password_flag
            || self.username.is_some()
            || self.email.is_some();

        let mode = if self.web {
            LoginMode::Web
        } else if want_password {
            LoginMode::Password
        } else if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
            // Interactive picker — TASKS/022-device-login.md §1.1.
            let choice = inquire::Select::new(
                "How would you like to authenticate Forest?",
                vec!["Login with a web browser  (recommended)", "Login with a password"],
            )
            .prompt()?;
            if choice.starts_with("Login with a web") {
                LoginMode::Web
            } else {
                LoginMode::Password
            }
        } else {
            eprintln!(
                "Defaulting to web login; pass --password for the legacy username/password flow."
            );
            LoginMode::Web
        };

        match mode {
            LoginMode::Web => login_web::run(state).await,
            LoginMode::Password => self.execute_password(state).await,
        }
    }

    async fn execute_password(&self, state: &State) -> anyhow::Result<()> {
        let identifier = match (&self.username, &self.email) {
            (Some(username), _) => login_request::Identifier::Username(username.clone()),
            (_, Some(email)) => login_request::Identifier::Email(email.clone()),
            (None, None) => {
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

        let password = if let Ok(p) = std::env::var("FOREST_PASSWORD") {
            p
        } else {
            inquire::Password::new("Password:")
                .with_display_mode(inquire::PasswordDisplayMode::Masked)
                .without_confirmation()
                .prompt()?
        };

        let resp = state
            .grpc_client()
            .login(identifier, &password)
            .await
            .context("failed to login")?;

        let (user, tokens) = if resp.mfa_required {
            eprintln!("Two-factor authentication required.");
            let code = inquire::Text::new("TOTP code:")
                .with_placeholder("123456")
                .prompt()?;

            let mfa_resp = state
                .grpc_client()
                .verify_login_mfa(&resp.mfa_session_token, &code)
                .await
                .context("MFA verification failed")?;

            (
                mfa_resp.user.context("no user in MFA response")?,
                mfa_resp.tokens.context("no tokens in MFA response")?,
            )
        } else {
            (
                resp.user.context("no user in login response")?,
                resp.tokens.context("no tokens in login response")?,
            )
        };
        eprintln!("Logged in as {} ({})", user.username, user.user_id);

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

enum LoginMode {
    Web,
    Password,
}
