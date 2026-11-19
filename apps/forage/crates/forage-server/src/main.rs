mod auth;
mod build_info;
mod checks;
mod cli;
mod compute_grpc;
mod email_consumer;
mod forest_client;
mod oidc;
mod notification_consumer;
mod notification_ingester;
mod manifest_view;
mod notification_worker;
mod http_metrics;
mod page_timing;
mod pretty_json;
mod providers;
mod recipients;
mod routes;
mod serve_grpc;
mod serve_http;
mod session_reaper;
mod state;
mod templates;
mod views;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

fn env_var_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

use forage_core::session::{FileSessionStore, SessionStore};
use forage_db::PgSessionStore;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use clap::Parser;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::forest_client::GrpcForestClient;
use crate::state::AppState;
use crate::templates::TemplateEngine;

async fn maintenance_page() -> Response {
    use sha2::Digest;
    let hash = format!("{:x}", sha2::Sha256::digest(MAINTENANCE_CSS.as_bytes()));
    let body = views::pages::maintenance::render(&views::catalogue::MaintenancePage {
        css_href: format!("{MAINTENANCE_CSS_PATH}?v={}", &hash[..12]),
    });

    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::RETRY_AFTER, "300"),
        ],
        Html(body),
    )
        .into_response()
}

const MAINTENANCE_CSS_PATH: &str = "/static/css/maintenance.css";
const MAINTENANCE_CSS: &str = include_str!("../../../static/css/maintenance.css");

async fn maintenance_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        MAINTENANCE_CSS,
    )
        .into_response()
}

pub fn build_maintenance_router(templates: TemplateEngine) -> Router {
    Router::new()
        .route(MAINTENANCE_CSS_PATH, axum::routing::get(maintenance_css))
        .fallback(maintenance_page)
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(templates)
        .merge(nostatus::axum_routes(nostatus::global()))
}

async fn fallback_404(State(state): State<AppState>) -> Response {
    let html = views::catalogue::error_page(
        &state.templates,
        404,
        "Page not found",
        "The page you're looking for doesn't exist.",
    );
    (StatusCode::NOT_FOUND, Html(html)).into_response()
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(routes::router())
        .nest_service("/static", ServeDir::new("static"))
        .fallback(fallback_404)
        .route_layer(axum::middleware::from_fn(http_metrics::layer))
        .layer(axum::middleware::from_fn(page_timing::layer))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
        .merge(nostatus::axum_routes(nostatus::global()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _otel = canopy_otel::init();

    let config = cli::Command::parse();

    if config.maintenance_mode {
        tracing::warn!("maintenance mode enabled; application routes return safe defaults");
    }

    let forest_endpoint =
        std::env::var("FOREST_SERVER_URL").unwrap_or_else(|_| "http://localhost:4040".into());
    tracing::info!("connecting to forest-server at {forest_endpoint}");

    let mut forest_client = GrpcForestClient::connect_lazy(&forest_endpoint)?;
    match std::env::var("FOREST_SERVICE_ACCOUNT_API_KEY") {
        Ok(service_key) if !service_key.is_empty() => {
            forest_client = forest_client.with_service_account_key(service_key);
        }
        _ => {
            tracing::warn!(
                "FOREST_SERVICE_ACCOUNT_API_KEY not set — \
                 CLI device login (/device) and any service-account-only \
                 forest RPC will be disabled."
            );
        }
    }
    let template_engine = TemplateEngine::new()?;

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let mut mad = notmad::Mad::builder();

    let (sessions, integration_store, magic_link_store, oauth_state_store): (
        Arc<dyn SessionStore>,
        Option<Arc<dyn forage_core::integrations::IntegrationStore>>,
        Option<Arc<dyn forage_core::auth::magic_link::MagicLinkStore>>,
        Option<Arc<dyn forage_core::auth::oauth_state::OAuthStateStore>>,
    );
    let state_profile_pictures: Option<Arc<forage_db::PgProfilePictureStore>>;
    let health_db: Option<sqlx::PgPool>;

    if let Ok(database_url) = std::env::var("DATABASE_URL") {
        tracing::info!("using PostgreSQL session store");
        let pool = sqlx::PgPool::connect(&database_url).await?;
        forage_db::migrate(&pool).await?;

        health_db = Some(pool.clone());

        let pg_store = Arc::new(PgSessionStore::new(pool.clone()));

        let encryption_key = std::env::var("INTEGRATION_ENCRYPTION_KEY").unwrap_or_else(|_| {
            tracing::warn!(
                "INTEGRATION_ENCRYPTION_KEY not set — using default key (not safe for production)"
            );
            "forage-dev-key-not-for-production!!".to_string()
        });
        let pg_integrations = Arc::new(forage_db::PgIntegrationStore::new(
            pool.clone(),
            encryption_key.into_bytes(),
        ));
        let pg_magic_link =
            Arc::new(forage_db::PgMagicLinkStore::new(pool.clone()));
        let pg_oauth_state =
            Arc::new(forage_db::PgOAuthStateStore::new(pool.clone()));
        let pg_profile_pictures =
            Arc::new(forage_db::PgProfilePictureStore::new(pool));

        mad.add(session_reaper::PgSessionReaper {
            store: pg_store.clone(),
            max_inactive_days: 30,
        });

        sessions = pg_store;
        integration_store =
            Some(pg_integrations as Arc<dyn forage_core::integrations::IntegrationStore>);
        magic_link_store =
            Some(pg_magic_link as Arc<dyn forage_core::auth::magic_link::MagicLinkStore>);
        oauth_state_store = Some(
            pg_oauth_state as Arc<dyn forage_core::auth::oauth_state::OAuthStateStore>,
        );
        state_profile_pictures = Some(pg_profile_pictures);
    } else {
        let session_dir = std::env::var("SESSION_DIR").unwrap_or_else(|_| "target/sessions".into());
        tracing::info!(
            "using file session store at {session_dir} (set DATABASE_URL for PostgreSQL)"
        );
        let file_store =
            Arc::new(FileSessionStore::new(&session_dir).expect("failed to create session dir"));

        mad.add(session_reaper::FileSessionReaper {
            store: file_store.clone(),
        });

        sessions = file_store as Arc<dyn SessionStore>;
        integration_store = None;
        magic_link_store = None;
        oauth_state_store = Some(Arc::new(
            forage_core::auth::oauth_state::InMemoryOAuthStateStore::new(),
        ));
        state_profile_pictures = None;
        health_db = None;
    };

    mad.add(checks::Checks { db: health_db });

    let forest_client = Arc::new(forest_client);

    let forage_host = std::env::var("FORAGE_HOST")
        .unwrap_or_else(|_| format!("http://localhost:{port}"));

    let mut state = AppState::new(
        template_engine,
        forest_client.clone(),
        forest_client.clone(),
        sessions,
    )
    .with_grpc_client(forest_client.clone())
    .with_registry_client(forest_client.clone())
    .with_oauth_apps_client(forest_client.clone())
    .with_forage_host(forage_host.clone());

    if let Some(key) = forest_client.service_account_key() {
        state = state.with_service_account_key(key.to_string());
    }

    if let Some(store) = state_profile_pictures {
        state = state.with_profile_picture_store(store);
    }

    if let (Some(client_id), Some(client_secret)) = (
        env_var_nonempty("SLACK_CLIENT_ID"),
        env_var_nonempty("SLACK_CLIENT_SECRET"),
    ) {
        tracing::info!("Slack OAuth enabled");
        state = state.with_slack_config(crate::state::SlackConfig {
            client_id,
            client_secret,
            redirect_host: forage_host.clone(),
        });
    }

    state = state.with_slack_api(
        crate::providers::SlackApi::new(
            env_var_nonempty("SLACK_API_BASE_URL")
                .unwrap_or_else(|| crate::providers::SLACK_API_BASE.to_string()),
        ),
        env_var_nonempty("SLACK_AUTHORIZE_URL")
            .unwrap_or_else(|| crate::providers::SLACK_AUTHORIZE_URL.to_string()),
    );
    if let Some(secret) = env_var_nonempty("SLACK_SIGNING_SECRET") {
        state = state.with_slack_signing_secret(secret);
    }

    if let (Some(app_id), Some(slug), Some(private_key_pem), Some(client_id), Some(client_secret)) = (
        env_var_nonempty("GITHUB_APP_ID"),
        env_var_nonempty("GITHUB_APP_SLUG"),
        env_var_nonempty("GITHUB_APP_PRIVATE_KEY"),
        env_var_nonempty("GITHUB_APP_CLIENT_ID"),
        env_var_nonempty("GITHUB_APP_CLIENT_SECRET"),
    ) {
        let app = crate::providers::GitHubApp::new(crate::providers::GitHubAppConfig {
            app_id,
            slug,
            client_id,
            client_secret,
            webhook_secret: env_var_nonempty("GITHUB_APP_WEBHOOK_SECRET").unwrap_or_default(),
            private_key_pem,
            api_base: env_var_nonempty("GITHUB_API_BASE_URL")
                .unwrap_or_else(|| "https://api.github.com".into()),
            web_base: env_var_nonempty("GITHUB_WEB_BASE_URL")
                .unwrap_or_else(|| "https://github.com".into()),
        })
        .map_err(|e| anyhow::anyhow!(e))?;
        tracing::info!("GitHub organisation connections enabled");
        state = state.with_github_app(std::sync::Arc::new(app));
    }

    if let Some(service_token) = forest_client.service_account_key() {
        state = state.with_recipient_directory(std::sync::Arc::new(
            crate::recipients::ForestRecipients {
                grpc: forest_client.clone(),
                service_token: service_token.to_string(),
            },
        ));
    }

    if let (Some(client_id), Some(client_secret)) = (
        env_var_nonempty("GOOGLE_CLIENT_ID"),
        env_var_nonempty("GOOGLE_CLIENT_SECRET"),
    ) {
        tracing::info!("Google OAuth enabled");
        let google_config = crate::state::GoogleOAuthConfig {
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
            redirect_host: forage_host.clone(),
        };
        let exchange = crate::oidc::GoogleOidcExchange::new(client_id, client_secret, forage_host.clone());
        state = state
            .with_google_oauth_config(google_config)
            .with_google_oidc_exchange(std::sync::Arc::new(exchange));
    }

    if let (Some(client_id), Some(client_secret)) = (
        env_var_nonempty("GITHUB_APP_CLIENT_ID"),
        env_var_nonempty("GITHUB_APP_CLIENT_SECRET"),
    ) {
        tracing::info!("GitHub OAuth enabled");
        let github_config = crate::state::GitHubOAuthConfig {
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
            redirect_host: forage_host.clone(),
        };
        let exchange = crate::oidc::GitHubOidcExchange::new(client_id, client_secret);
        state = state
            .with_github_oauth_config(github_config)
            .with_github_oidc_exchange(std::sync::Arc::new(exchange));
    }

    let nats_jetstream = if let Ok(nats_url) = std::env::var("NATS_URL") {
        match async_nats::connect(&nats_url).await {
            Ok(client) => {
                tracing::info!("connected to NATS at {nats_url}");
                Some(async_nats::jetstream::new(client))
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to connect to NATS — falling back to direct dispatch");
                None
            }
        }
    } else {
        None
    };

    if let Some(ref store) = integration_store {
        state = state.with_integration_store(store.clone());

        if let Some(service_token) = forest_client.service_account_key().map(String::from) {
            let forage_url = forage_host.clone();

            if let Some(ref js) = nats_jetstream {
                tracing::info!("starting notification pipeline (JetStream)");
                let grpc_for_consumer = forest_client.clone();
                let token_for_consumer = service_token.clone();
                mad.add(notification_ingester::NotificationIngester {
                    grpc: forest_client,
                    jetstream: js.clone(),
                    service_token,
                });
                mad.add(notification_consumer::NotificationConsumer {
                    jetstream: js.clone(),
                    store: store.clone(),
                    forage_url,
                    grpc: grpc_for_consumer,
                    service_token: token_for_consumer,
                    slack: state.slack_api.clone(),
                });
            } else {
                tracing::warn!(
                    "NATS_URL not set — using direct notification dispatch (no durability)"
                );
                mad.add(notification_worker::NotificationListener {
                    grpc: forest_client,
                    store: store.clone(),
                    service_token,
                    forage_url,
                    slack: state.slack_api.clone(),
                });
            }
        } else {
            tracing::warn!(
                "FOREST_SERVICE_ACCOUNT_API_KEY not set — notification listener disabled"
            );
        }
    }

    if let Some(store) = oauth_state_store {
        state = state.with_oauth_state_store(store);
    }

    if let Some(store) = magic_link_store {
        state = state.with_magic_link_store(store);

        if let Some(ref js) = nats_jetstream {
            if let Some(smtp_config) = email_consumer::SmtpConfig::from_env() {
                tracing::info!("email consumer enabled (NATS + SMTP)");
                mad.add(email_consumer::EmailConsumer {
                    jetstream: js.clone(),
                    smtp_config,
                });
                state = state.with_email_jetstream(js.clone());
            }
        }
    }

    let compute_scheduler = Arc::new(forage_core::compute::InMemoryComputeScheduler::new());
    state = state.with_compute_scheduler(compute_scheduler.clone());

    let grpc_port: u16 = std::env::var("GRPC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4050);
    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], grpc_port));
    mad.add(serve_grpc::ServeGrpc {
        addr: grpc_addr,
        scheduler: compute_scheduler,
        maintenance_mode: config.maintenance_mode,
    });

    let mode = if config.maintenance_mode {
        serve_http::ServeMode::Maintenance(Box::new(state.templates.clone()))
    } else {
        serve_http::ServeMode::Application(Box::new(state))
    };
    mad.add(serve_http::ServeHttp { addr, mode });

    mad.cancellation(Some(Duration::from_secs(10)))
        .run()
        .await?;

    Ok(())
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod test_providers;
#[cfg(test)]
mod tests;
