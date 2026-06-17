use anyhow::Context;
use forest_grpc_interface::OAuthApp;
use serde::Serialize;
use tabled::Tabled;

use crate::{
    cli::output::{self, OutputFormat},
    grpc::GrpcClientState,
    state::State,
    user_state::UserStateLoaderState,
};

/// Manage organisation OAuth applications ("Sign in with Forest").
#[derive(clap::Parser)]
pub struct OAuthAppCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Register a new OAuth application
    Create(CreateCommand),
    /// List an organisation's OAuth applications
    List(ListCommand),
    /// Show a single OAuth application
    #[command(alias = "get")]
    Show(ShowCommand),
    /// Rotate an application's client secret (the old one stops working)
    RotateSecret(RotateSecretCommand),
    /// Delete an OAuth application (revokes all its tokens)
    Delete(DeleteCommand),
}

impl OAuthAppCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(
            self.commands,
            Commands::Create(_) | Commands::RotateSecret(_) | Commands::Delete(_)
        )
    }

    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(c) => c.execute(state, format).await,
            Commands::List(c) => c.execute(state, format).await,
            Commands::Show(c) => c.execute(state, format).await,
            Commands::RotateSecret(c) => c.execute(state, format).await,
            Commands::Delete(c) => c.execute(state, format).await,
        }
    }
}

// ─── Row view models ─────────────────────────────────────────────────

#[derive(Tabled, Serialize)]
struct AppRow {
    #[tabled(rename = "App ID")]
    app_id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Client ID")]
    client_id: String,
    #[tabled(rename = "Scopes")]
    scopes: String,
    #[tabled(rename = "Redirect URIs")]
    redirect_uris: String,
}

impl From<OAuthApp> for AppRow {
    fn from(a: OAuthApp) -> Self {
        AppRow {
            app_id: a.app_id,
            name: a.name,
            client_id: a.client_id,
            scopes: a.scopes.join(" "),
            redirect_uris: a.redirect_uris.join(", "),
        }
    }
}

async fn require_login(state: &State) -> anyhow::Result<()> {
    state
        .user_state()
        .get_state()
        .await?
        .context("you must be logged in")?;
    Ok(())
}

// ─── create ──────────────────────────────────────────────────────────

#[derive(clap::Parser)]
pub struct CreateCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: String,
    /// Display name shown on the consent screen
    #[arg(long)]
    name: String,
    /// Short description (optional)
    #[arg(long, default_value = "")]
    description: String,
    /// Homepage URL (optional)
    #[arg(long, default_value = "")]
    homepage_url: String,
    /// Allowed redirect URI (repeatable; at least one required)
    #[arg(long = "redirect-uri", required = true)]
    redirect_uri: Vec<String>,
    /// Requested scope (repeatable). Defaults to: openid profile
    #[arg(long)]
    scope: Vec<String>,
}

#[derive(Tabled, Serialize)]
struct CreatedRow {
    #[tabled(rename = "App ID")]
    app_id: String,
    #[tabled(rename = "Client ID")]
    client_id: String,
    #[tabled(rename = "Client Secret (shown once)")]
    client_secret: String,
}

impl CreateCommand {
    async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        let scopes = if self.scope.is_empty() {
            vec!["openid".to_string(), "profile".to_string()]
        } else {
            self.scope.clone()
        };

        let resp = state
            .grpc_client()
            .create_oauth_app(
                &org_id,
                &self.name,
                &self.description,
                &self.homepage_url,
                self.redirect_uri.clone(),
                scopes,
            )
            .await
            .context("failed to create oauth app")?;
        let app = resp.app.context("no app in response")?;

        let rows = vec![CreatedRow {
            app_id: app.app_id,
            client_id: app.client_id,
            client_secret: resp.client_secret,
        }];
        print!("{}", output::render(format, &rows));
        eprintln!("Save the client secret now — it will not be shown again.");
        Ok(())
    }
}

// ─── list ────────────────────────────────────────────────────────────

#[derive(clap::Parser)]
pub struct ListCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: String,
}

impl ListCommand {
    async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        let apps = state
            .grpc_client()
            .list_oauth_apps(&org_id)
            .await
            .context("failed to list oauth apps")?;
        let rows: Vec<AppRow> = apps.into_iter().map(AppRow::from).collect();
        print!("{}", output::render(format, &rows));
        Ok(())
    }
}

// ─── show ────────────────────────────────────────────────────────────

#[derive(clap::Parser)]
pub struct ShowCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: String,
    /// OAuth application ID
    #[arg(long)]
    id: String,
}

impl ShowCommand {
    async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        let app = state
            .grpc_client()
            .get_oauth_app(&org_id, &self.id)
            .await
            .context("failed to get oauth app")?
            .ok_or_else(|| anyhow::anyhow!("oauth app '{}' not found", self.id))?;
        let rows = vec![AppRow::from(app)];
        print!("{}", output::render(format, &rows));
        Ok(())
    }
}

// ─── rotate-secret ───────────────────────────────────────────────────

#[derive(clap::Parser)]
pub struct RotateSecretCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: String,
    /// OAuth application ID
    #[arg(long)]
    id: String,
}

impl RotateSecretCommand {
    async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        let resp = state
            .grpc_client()
            .rotate_oauth_app_secret(&org_id, &self.id)
            .await
            .context("failed to rotate oauth app secret")?;
        let app = resp.app.context("no app in response")?;
        let rows = vec![CreatedRow {
            app_id: app.app_id,
            client_id: app.client_id,
            client_secret: resp.client_secret,
        }];
        print!("{}", output::render(format, &rows));
        eprintln!("Save the new client secret now — it will not be shown again.");
        Ok(())
    }
}

// ─── delete ──────────────────────────────────────────────────────────

#[derive(clap::Parser)]
pub struct DeleteCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: String,
    /// OAuth application ID
    #[arg(long)]
    id: String,
}

impl DeleteCommand {
    async fn execute(&self, state: &State, _format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        state
            .grpc_client()
            .delete_oauth_app(&org_id, &self.id)
            .await
            .context("failed to delete oauth app")?;
        eprintln!("Deleted OAuth application {}", self.id);
        Ok(())
    }
}
