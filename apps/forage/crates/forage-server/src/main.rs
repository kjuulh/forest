mod auth;
mod compute_grpc;
mod email_consumer;
mod forest_client;
mod oidc;
mod notification_consumer;
mod notification_ingester;
mod manifest_view;
mod notification_worker;
mod pretty_json;
mod routes;
mod serve_grpc;
mod serve_http;
mod session_reaper;
mod state;
mod templates;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// Read an env var, treating "unset" and "set to empty string" the same way.
///
/// The infra-platform layer wires forage's OAuth / SMTP / service-token
/// slots through a console-managed Secrets Manager secret. Slots that the
/// operator hasn't populated arrive as empty strings (Secrets Manager
/// can't store `None` for a JSON value), so we have to filter those out
/// here — otherwise `if let Ok(...)` happily picks them up and forage
/// boots with a half-configured Slack/Google/etc. client that fails at
/// runtime with a worse error.
fn env_var_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

use forage_core::session::{FileSessionStore, SessionStore};
use forage_db::PgSessionStore;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::forest_client::GrpcForestClient;
use crate::state::AppState;
use crate::templates::TemplateEngine;

async fn fallback_404(State(state): State<AppState>) -> Response {
    let html = state.templates.render(
        "pages/error.html.jinja",
        context! {
            title => "Not Found - Forest",
            description => "The page you're looking for doesn't exist.",
            status => 404u16,
            heading => "Page not found",
            message => "The page you're looking for doesn't exist.",
        },
    );
    match html {
        Ok(body) => (StatusCode::NOT_FOUND, Html(body)).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(routes::router())
        .nest_service("/static", ServeDir::new("static"))
        .fallback(fallback_404)
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // OTLP traces + logs + metrics when `OTEL_SERVICE_NAME` is set.
    // Falls back to pretty fmt-only when unset, matching the previous
    // default. The guard flushes on drop, so hold it until end of main.
    let _otel = canopy_otel::init();

    let forest_endpoint =
        std::env::var("FOREST_SERVER_URL").unwrap_or_else(|_| "http://localhost:4040".into());
    tracing::info!("connecting to forest-server at {forest_endpoint}");

    let mut forest_client = GrpcForestClient::connect_lazy(&forest_endpoint)?;
    match std::env::var("FOREST_SERVICE_ACCOUNT_API_KEY") {
        Ok(service_key) if !service_key.is_empty() => {
            forest_client = forest_client.with_service_account_key(service_key);
        }
        _ => {
            // The CLI device-login flow and the OAuth callback path
            // both require this key to call forest-server's
            // service-account-only RPCs. Without it, `/device` falls
            // back to an explicit "not configured" message rather than
            // pretending the code is invalid.
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

    // Build components based on available configuration
    let mut mad = notmad::Mad::builder();

    // Session store + integration store: PostgreSQL if DATABASE_URL is set
    let (sessions, integration_store, magic_link_store, oauth_state_store): (
        Arc<dyn SessionStore>,
        Option<Arc<dyn forage_core::integrations::IntegrationStore>>,
        Option<Arc<dyn forage_core::auth::magic_link::MagicLinkStore>>,
        Option<Arc<dyn forage_core::auth::oauth_state::OAuthStateStore>>,
    );
    let state_profile_pictures: Option<Arc<forage_db::PgProfilePictureStore>>;

    if let Ok(database_url) = std::env::var("DATABASE_URL") {
        tracing::info!("using PostgreSQL session store");
        let pool = sqlx::PgPool::connect(&database_url).await?;
        forage_db::migrate(&pool).await?;

        let pg_store = Arc::new(PgSessionStore::new(pool.clone()));

        // Integration store (uses same pool)
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

        // Session reaper component
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

        // File session reaper component
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
    };

    let forest_client = Arc::new(forest_client);

    // Public URL of this Forest instance (used for OAuth redirect URIs, etc.)
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
    .with_forage_host(forage_host.clone());

    if let Some(key) = forest_client.service_account_key() {
        state = state.with_service_account_key(key.to_string());
    }

    if let Some(store) = state_profile_pictures {
        state = state.with_profile_picture_store(store);
    }

    // Slack OAuth config (optional, enables "Add to Slack" button)
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

    // Google OAuth config (optional, enables "Continue with Google" button)
    // Forest handles the full OIDC exchange — needs both client_id and client_secret.
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

    // GitHub OAuth config (optional, enables "Continue with GitHub" button)
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

    // NATS JetStream connection (optional, enables durable notification delivery)
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

        // The notification listener subscribes to forest's stream as a
        // service account — it sets `organisation: None, project: None`
        // so it gets every notification regardless of recipient. That's
        // exactly the trust model `FOREST_SERVICE_ACCOUNT_API_KEY`
        // already represents, so we reuse it here instead of carrying a
        // second forest-side credential (was `FORAGE_SERVICE_TOKEN`).
        //
        // Forest's `poll_notifications` only references the caller's
        // user_id in the preferences NOT-EXISTS subquery (skip-if-muted);
        // a service-account caller has no preferences, so nothing gets
        // muted and the listener receives the full stream — identical
        // to the previous PAT-based behaviour, with one fewer secret to
        // manage.
        if let Some(service_token) = forest_client.service_account_key().map(String::from) {
            let forage_url = forage_host.clone();

            if let Some(ref js) = nats_jetstream {
                // JetStream mode: ingester publishes, consumer dispatches
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
                });
            } else {
                // Fallback: direct dispatch (no durability)
                tracing::warn!(
                    "NATS_URL not set — using direct notification dispatch (no durability)"
                );
                mad.add(notification_worker::NotificationListener {
                    grpc: forest_client,
                    store: store.clone(),
                    service_token,
                    forage_url,
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

    // Magic link store + email consumer
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

    // Compute scheduler (mock for now — simulates container lifecycle)
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
    });

    // HTTP server component
    mad.add(serve_http::ServeHttp { addr, state });

    mad.cancellation(Some(Duration::from_secs(10)))
        .run()
        .await?;

    Ok(())
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
