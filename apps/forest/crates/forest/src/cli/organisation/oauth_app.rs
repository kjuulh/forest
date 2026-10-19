use anyhow::Context;
use forest_grpc_interface::{OAuthApp, UpdateOAuthAppRequest};
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
    /// Change an application: grants, scopes, redirect URIs or details
    Update(UpdateCommand),
    /// Rotate an application's client secret (the old one stops working)
    RotateSecret(RotateSecretCommand),
    /// Delete an OAuth application (revokes all its tokens)
    Delete(DeleteCommand),
}

impl OAuthAppCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(
            self.commands,
            Commands::Create(_)
                | Commands::Update(_)
                | Commands::RotateSecret(_)
                | Commands::Delete(_)
        )
    }

    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(c) => c.execute(state, format).await,
            Commands::List(c) => c.execute(state, format).await,
            Commands::Show(c) => c.execute(state, format).await,
            Commands::Update(c) => c.execute(state, format).await,
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
    #[tabled(rename = "Public")]
    public: bool,
    #[tabled(rename = "Scopes")]
    scopes: String,
    #[tabled(rename = "Grants")]
    grant_types: String,
    #[tabled(rename = "Redirect URIs")]
    redirect_uris: String,
}

impl From<OAuthApp> for AppRow {
    fn from(a: OAuthApp) -> Self {
        AppRow {
            app_id: a.app_id,
            name: a.name,
            client_id: a.client_id,
            public: a.public,
            scopes: a.scopes.join(" "),
            grant_types: a.grant_types.join(" "),
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
    /// Requested scope (repeatable). Defaults to: openid profile (profile
    /// alone with --public)
    #[arg(long)]
    scope: Vec<String>,
    /// OAuth grant this app may use (repeatable): `authorization_code`
    /// for "Sign in with Forest", `client_credentials` for a service
    /// calling as itself, `urn:ietf:params:oauth:grant-type:device_code`
    /// for a CLI that signs a person in with a code they approve on any
    /// device (RFC 8628; works over SSH).
    ///
    /// Pass both to get both, the way a GitHub App acts for a user and
    /// as itself:
    ///
    ///   --grant-type authorization_code --grant-type client_credentials
    ///
    /// Defaults to `authorization_code` — an app only becomes
    /// machine-capable by asking for it.
    #[arg(long = "grant-type")]
    grant_type: Vec<String>,
    /// Register a public client: a CLI or other native app that cannot
    /// keep a secret (RFC 8252).
    ///
    /// No client secret is issued. The app may only use
    /// `authorization_code` (every authorization must carry a PKCE S256
    /// challenge) and the device grant, and it may not request `openid`. A loopback redirect URI
    /// (`http://127.0.0.1/<path>`) matches any port, so the app can listen
    /// on whatever port is free.
    #[arg(long)]
    public: bool,
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

#[derive(Tabled, Serialize)]
struct CreatedPublicRow {
    #[tabled(rename = "App ID")]
    app_id: String,
    #[tabled(rename = "Client ID")]
    client_id: String,
}

impl CreateCommand {
    async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        let scopes = if self.scope.is_empty() && self.public {
            // A public client may not hold `openid`.
            vec!["profile".to_string()]
        } else if self.scope.is_empty() {
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
                self.grant_type.clone(),
                self.public,
            )
            .await
            .context("failed to create oauth app")?;
        let app = resp.app.context("no app in response")?;

        if app.public {
            let rows = vec![CreatedPublicRow {
                app_id: app.app_id,
                client_id: app.client_id,
            }];
            print!("{}", output::render(format, &rows));
            eprintln!("Public client: no client secret. Authorize with PKCE (S256).");
            return Ok(());
        }

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

// ─── update ──────────────────────────────────────────────────────────

/// Change an application. Only what is given changes; the rest is read back
/// from Forest first, since an update replaces every field. Removing a grant
/// takes effect at once: in-flight codes and refresh tokens for it stop
/// working.
///
///   forest organisation oauth-app update --org understory --id <app-id> ///     --add-grant-type urn:ietf:params:oauth:grant-type:device_code
#[derive(clap::Parser)]
pub struct UpdateCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: String,
    /// OAuth application ID
    #[arg(long)]
    id: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    homepage_url: Option<String>,
    /// Replace the redirect URIs (repeatable)
    #[arg(long = "redirect-uri")]
    redirect_uri: Vec<String>,
    /// Add a scope (repeatable)
    #[arg(long)]
    add_scope: Vec<String>,
    /// Remove a scope (repeatable)
    #[arg(long)]
    remove_scope: Vec<String>,
    /// Add a grant (repeatable): `authorization_code`, `client_credentials`
    /// or `urn:ietf:params:oauth:grant-type:device_code`
    #[arg(long)]
    add_grant_type: Vec<String>,
    /// Remove a grant (repeatable)
    #[arg(long)]
    remove_grant_type: Vec<String>,
}

impl UpdateCommand {
    fn request(&self, organisation_id: &str, app: OAuthApp) -> UpdateOAuthAppRequest {
        fn edit(mut list: Vec<String>, add: &[String], remove: &[String]) -> Vec<String> {
            list.retain(|v| !remove.contains(v));
            for v in add {
                if !list.contains(v) {
                    list.push(v.clone());
                }
            }
            list
        }
        UpdateOAuthAppRequest {
            organisation_id: organisation_id.to_string(),
            app_id: app.app_id,
            name: self.name.clone().unwrap_or(app.name),
            description: self.description.clone().unwrap_or(app.description),
            homepage_url: self.homepage_url.clone().unwrap_or(app.homepage_url),
            redirect_uris: if self.redirect_uri.is_empty() {
                app.redirect_uris
            } else {
                self.redirect_uri.clone()
            },
            scopes: edit(app.scopes, &self.add_scope, &self.remove_scope),
            grant_types: edit(
                app.grant_types,
                &self.add_grant_type,
                &self.remove_grant_type,
            ),
        }
    }

    async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        require_login(state).await?;
        let org_id = super::member::resolve_org_id(state, &self.org).await?;
        let client = state.grpc_client();
        let app = client
            .get_oauth_app(&org_id, &self.id)
            .await
            .context("failed to get oauth app")?
            .ok_or_else(|| anyhow::anyhow!("oauth app '{}' not found", self.id))?;
        let request = self.request(&org_id, app);
        if request.grant_types.is_empty() {
            anyhow::bail!("an app needs at least one grant type");
        }
        let app = client
            .update_oauth_app(request)
            .await
            .context("failed to update oauth app")?
            .ok_or_else(|| anyhow::anyhow!("oauth app '{}' not found", self.id))?;
        print!("{}", output::render(format, &[AppRow::from(app)]));
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    fn app() -> OAuthApp {
        OAuthApp {
            app_id: "app-1".into(),
            name: "datactl".into(),
            description: "kubectl for the data gateway".into(),
            redirect_uris: vec!["http://127.0.0.1/callback".into()],
            scopes: vec!["datagateway:apply".into(), "datagateway:read".into()],
            grant_types: vec!["authorization_code".into()],
            public: true,
            ..Default::default()
        }
    }

    fn update(args: &[&str]) -> UpdateCommand {
        UpdateCommand::try_parse_from(
            ["update", "--org", "understory", "--id", "app-1"]
                .iter()
                .chain(args),
        )
        .unwrap()
    }

    #[test]
    fn adding_a_grant_keeps_everything_else_the_app_had() {
        let req = update(&[
            "--add-grant-type",
            "urn:ietf:params:oauth:grant-type:device_code",
        ])
        .request("org-1", app());
        assert_eq!(
            req.grant_types,
            vec![
                "authorization_code",
                "urn:ietf:params:oauth:grant-type:device_code"
            ]
        );
        assert_eq!(req.scopes, app().scopes);
        assert_eq!(req.redirect_uris, app().redirect_uris);
        assert_eq!(req.name, "datactl");
        assert_eq!(req.description, "kubectl for the data gateway");
    }

    #[test]
    fn removing_and_adding_scopes_and_grants_edits_only_those() {
        let req = update(&[
            "--remove-scope",
            "datagateway:apply",
            "--add-scope",
            "datagateway:read",
            "--remove-grant-type",
            "authorization_code",
            "--add-grant-type",
            "urn:ietf:params:oauth:grant-type:device_code",
            "--redirect-uri",
            "http://localhost/cb",
        ])
        .request("org-1", app());
        assert_eq!(req.scopes, vec!["datagateway:read"]);
        assert_eq!(
            req.grant_types,
            vec!["urn:ietf:params:oauth:grant-type:device_code"]
        );
        assert_eq!(req.redirect_uris, vec!["http://localhost/cb"]);
    }
}
