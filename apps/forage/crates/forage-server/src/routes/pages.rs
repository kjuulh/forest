use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use minijinja::context;

use crate::auth::MaybeSession;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(landing))
        .route("/pricing", get(pricing))
}

async fn landing(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
) -> Result<Response, axum::http::StatusCode> {
    if maybe_session.session.is_some() {
        return Ok(Redirect::to("/dashboard").into_response());
    }

    let html = state
        .templates
        .render("pages/landing.html.jinja", context! {
            title => "Forest - release on your runtime or ours",
            description => "Release applications on Forest Runtime or customer infrastructure through one versioned delivery workflow.",
            is_landing => true,
        })
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Html(html).into_response())
}

async fn pricing(State(state): State<AppState>) -> Result<Html<String>, axum::http::StatusCode> {
    let html = state
        .templates
        .render("pages/pricing.html.jinja", context! {
            title => "Pricing - Forest",
            description => "Private preview terms and future pricing principles for Forest.",
        })
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Html(html))
}

