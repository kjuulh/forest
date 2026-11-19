use axum::Router;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use forage_ui::chrome::PublicNav;

use crate::auth::MaybeSession;
use crate::state::AppState;
use crate::views::catalogue::{VisitorPage, visitor};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(landing))
        .route("/pricing", get(pricing))
}

async fn landing(State(state): State<AppState>, maybe_session: MaybeSession) -> Response {
    if maybe_session.session.is_some() {
        return Redirect::to("/dashboard").into_response();
    }

    let page = VisitorPage {
        chrome: visitor(
            &state.templates,
            "Forest - your developer platform",
            "Push a forest.cue manifest, get production infrastructure.",
            PublicNav {
                is_landing: true,
                ..Default::default()
            },
        ),
    };
    Html(crate::views::pages::landing::render(&page)).into_response()
}

async fn pricing(State(state): State<AppState>) -> Html<String> {
    let page = VisitorPage {
        chrome: visitor(
            &state.templates,
            "Pricing - Forage",
            "Simple, transparent pricing. Pay only for what you use.",
            PublicNav::default(),
        ),
    };
    Html(crate::views::pages::pricing::render(&page))
}
