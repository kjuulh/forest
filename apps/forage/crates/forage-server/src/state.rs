use std::sync::Arc;

use crate::forest_client::GrpcForestClient;
use crate::templates::TemplateEngine;
use forage_core::auth::magic_link::MagicLinkStore;
use forage_core::auth::oauth_state::OAuthStateStore;
use forage_core::auth::{ForestAuth, OidcExchange};
use forage_core::compute::ComputeScheduler;
use forage_core::integrations::IntegrationStore;
use forage_core::integrations::direct::{RecipientDirectory, SlackDirectory};
use forage_core::platform::{ForestOAuthApps, ForestPlatform};
use forage_core::registry::ForestRegistry;
use forage_core::session::SessionStore;
use forage_db::PgProfilePictureStore;

#[derive(Clone)]
pub struct SlackConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_host: String,
}

#[derive(Clone)]
pub struct GoogleOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_host: String,
}

#[derive(Clone)]
pub struct GitHubOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_host: String,
}

#[derive(Clone)]
pub struct AppState {
    pub templates: TemplateEngine,
    pub forest_client: Arc<dyn ForestAuth>,
    pub platform_client: Arc<dyn ForestPlatform>,
    pub sessions: Arc<dyn SessionStore>,
    pub grpc_client: Option<Arc<GrpcForestClient>>,
    pub integration_store: Option<Arc<dyn IntegrationStore>>,
    pub slack_config: Option<SlackConfig>,
    pub google_oauth_config: Option<GoogleOAuthConfig>,
    pub google_oidc_exchange: Option<Arc<dyn OidcExchange>>,
    pub github_oauth_config: Option<GitHubOAuthConfig>,
    pub github_oidc_exchange: Option<Arc<dyn OidcExchange>>,
    pub magic_link_store: Option<Arc<dyn MagicLinkStore>>,
    pub oauth_state_store: Option<Arc<dyn OAuthStateStore>>,
    pub email_jetstream: Option<async_nats::jetstream::Context>,
    pub forage_host: String,
    pub compute_scheduler: Option<Arc<dyn ComputeScheduler>>,
    pub profile_picture_store: Option<Arc<PgProfilePictureStore>>,
    pub registry_client: Option<Arc<dyn ForestRegistry>>,
    pub oauth_apps_client: Option<Arc<dyn ForestOAuthApps>>,
    pub service_account_key: Option<String>,
    pub slack_api: crate::providers::SlackApi,
    pub slack_authorize_url: String,
    pub slack_signing_secret: Option<String>,
    pub slack_directory: Option<Arc<dyn SlackDirectory>>,
    pub recipient_directory: Option<Arc<dyn RecipientDirectory>>,
    pub github_app: Option<Arc<crate::providers::GitHubApp>>,
}

impl AppState {
    pub fn new(
        templates: TemplateEngine,
        forest_client: Arc<dyn ForestAuth>,
        platform_client: Arc<dyn ForestPlatform>,
        sessions: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            templates,
            forest_client,
            platform_client,
            sessions,
            grpc_client: None,
            integration_store: None,
            slack_config: None,
            google_oauth_config: None,
            google_oidc_exchange: None,
            github_oauth_config: None,
            github_oidc_exchange: None,
            magic_link_store: None,
            oauth_state_store: None,
            email_jetstream: None,
            forage_host: String::new(),
            compute_scheduler: None,
            profile_picture_store: None,
            registry_client: None,
            oauth_apps_client: None,
            service_account_key: None,
            slack_api: crate::providers::SlackApi::default(),
            slack_authorize_url: crate::providers::SLACK_AUTHORIZE_URL.to_string(),
            slack_signing_secret: None,
            slack_directory: None,
            recipient_directory: None,
            github_app: None,
        }
    }

    pub fn with_grpc_client(mut self, client: Arc<GrpcForestClient>) -> Self {
        self.grpc_client = Some(client);
        self
    }

    pub fn with_integration_store(mut self, store: Arc<dyn IntegrationStore>) -> Self {
        self.integration_store = Some(store);
        self
    }

    pub fn with_slack_config(mut self, config: SlackConfig) -> Self {
        self.slack_config = Some(config);
        self
    }

    pub fn with_google_oauth_config(mut self, config: GoogleOAuthConfig) -> Self {
        self.google_oauth_config = Some(config);
        self
    }

    pub fn with_google_oidc_exchange(mut self, exchange: Arc<dyn OidcExchange>) -> Self {
        self.google_oidc_exchange = Some(exchange);
        self
    }

    pub fn with_github_oauth_config(mut self, config: GitHubOAuthConfig) -> Self {
        self.github_oauth_config = Some(config);
        self
    }

    pub fn with_github_oidc_exchange(mut self, exchange: Arc<dyn OidcExchange>) -> Self {
        self.github_oidc_exchange = Some(exchange);
        self
    }

    pub fn with_magic_link_store(mut self, store: Arc<dyn MagicLinkStore>) -> Self {
        self.magic_link_store = Some(store);
        self
    }

    pub fn with_oauth_state_store(mut self, store: Arc<dyn OAuthStateStore>) -> Self {
        self.oauth_state_store = Some(store);
        self
    }

    pub fn with_email_jetstream(mut self, js: async_nats::jetstream::Context) -> Self {
        self.email_jetstream = Some(js);
        self
    }

    pub fn with_forage_host(mut self, host: String) -> Self {
        self.forage_host = host;
        self
    }

    pub fn with_compute_scheduler(mut self, scheduler: Arc<dyn ComputeScheduler>) -> Self {
        self.compute_scheduler = Some(scheduler);
        self
    }

    pub fn with_profile_picture_store(mut self, store: Arc<PgProfilePictureStore>) -> Self {
        self.profile_picture_store = Some(store);
        self
    }

    pub fn with_registry_client(mut self, client: Arc<dyn ForestRegistry>) -> Self {
        self.registry_client = Some(client);
        self
    }

    pub fn with_oauth_apps_client(mut self, client: Arc<dyn ForestOAuthApps>) -> Self {
        self.oauth_apps_client = Some(client);
        self
    }

    pub fn with_service_account_key(mut self, key: String) -> Self {
        self.service_account_key = Some(key);
        self
    }

    pub fn with_slack_api(mut self, api: crate::providers::SlackApi, authorize_url: String) -> Self {
        self.slack_directory = Some(Arc::new(api.clone()));
        self.slack_api = api;
        self.slack_authorize_url = authorize_url;
        self
    }

    pub fn with_slack_signing_secret(mut self, secret: String) -> Self {
        self.slack_signing_secret = Some(secret);
        self
    }

    pub fn with_slack_directory(mut self, directory: Arc<dyn SlackDirectory>) -> Self {
        self.slack_directory = Some(directory);
        self
    }

    pub fn with_recipient_directory(mut self, directory: Arc<dyn RecipientDirectory>) -> Self {
        self.recipient_directory = Some(directory);
        self
    }

    pub fn with_github_app(mut self, app: Arc<crate::providers::GitHubApp>) -> Self {
        self.github_app = Some(app);
        self
    }
}
