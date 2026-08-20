//! Sensitive destination metadata in the web UI.
//!
//! The platform withholds values for keys it considers credentials, so these
//! tests pin the two things that could still leak or break:
//!   * a withheld value must never appear in the rendered page, and
//!   * saving the metadata form must not blank out the withheld value.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::Destination;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

/// A terraform destination shaped like the one from DATA-575: benign config
/// visible, credentials withheld by the platform.
fn destination_with_withheld_keys() -> Destination {
    Destination {
        name: "platform-dev".into(),
        environment: "prod".into(),
        organisation: "testorg".into(),
        metadata: [
            ("tf_workspace".to_string(), "platform-dev".to_string()),
            ("aws_account_id".to_string(), "123456789012".to_string()),
        ]
        .into(),
        sensitive_keys: vec![
            "aws_secret_access_key".into(),
            "cloudflare_token".into(),
        ],
        dest_type: Some(forage_core::platform::DestinationType {
            organisation: "forest".into(),
            name: "terraform".into(),
            version: 1,
        }),
    }
}

fn state_with_destination() -> (
    crate::state::AppState,
    std::sync::Arc<forage_core::session::InMemorySessionStore>,
) {
    test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(MockPlatformBehavior {
            list_destinations_result: Some(Ok(vec![destination_with_withheld_keys()])),
            // The destinations page groups by environment; without a matching
            // one the destination renders in the "other destinations" list,
            // which shows no metadata at all and would pass vacuously.
            list_environments_result: Some(Ok(vec![forage_core::platform::Environment {
                id: "env-prod".into(),
                organisation: "testorg".into(),
                name: "prod".into(),
                description: None,
                sort_order: 0,
                created_at: "2026-03-08T00:00:00Z".into(),
            }])),
            ..Default::default()
        }),
    )
}

async fn body_of(response: axum::response::Response) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn detail_page_masks_withheld_keys_and_shows_the_rest() {
    let (state, sessions) = state_with_destination();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations/detail?name=platform-dev")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_of(response).await;

    // Withheld keys are named, and offer a reveal control.
    assert!(html.contains("aws_secret_access_key"), "key name should be listed");
    assert!(html.contains("cloudflare_token"));
    assert!(html.contains("reveal-btn"), "expected a reveal control");
    assert!(
        html.contains("preserve_sensitive_keys"),
        "expected the marker that keeps the value alive across a save"
    );

    // Non-sensitive values still render.
    assert!(html.contains("123456789012"));
    assert!(html.contains("platform-dev"));

    // The mock hands out `revealed-<key>` when asked; nothing should have asked.
    assert!(
        !html.contains("revealed-aws_secret_access_key")
            && !html.contains("revealed-cloudflare_token"),
        "a withheld value was rendered into the page"
    );
}

#[tokio::test]
async fn destinations_list_page_does_not_embed_withheld_values() {
    let (state, sessions) = state_with_destination();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_of(response).await;

    // Sanity: the export button really is on the page for this destination.
    assert!(html.contains("export-dest"), "expected the export control to render");

    // The export payload lives in a data attribute; it must carry the withheld
    // key names, not their values.
    assert!(html.contains("data-sensitive-keys"));
    assert!(html.contains("aws_secret_access_key"));
    assert!(
        !html.contains("revealed-cloudflare_token")
            && !html.contains("revealed-aws_secret_access_key"),
        "the export attribute must not carry a withheld value"
    );

    // Counts cover hidden keys too, so the page does not understate what is set.
    assert!(html.contains("4 keys"), "expected 2 visible + 2 hidden keys");
    assert!(html.contains("2 hidden"));
}

#[tokio::test]
async fn reveal_endpoint_returns_one_value_as_plain_text() {
    let (state, sessions) = state_with_destination();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations/detail/reveal?name=platform-dev&key=cloudflare_token")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store, max-age=0"),
        "a revealed credential must not be cached"
    );
    assert_eq!(body_of(response).await, "revealed-cloudflare_token");
}

#[tokio::test]
async fn reveal_endpoint_rejects_non_members() {
    let (state, sessions) = state_with_destination();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/otherorg/destinations/detail/reveal?name=platform-dev&key=cloudflare_token")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reveal_endpoint_rejects_non_admin_members() {
    let (state, sessions) = state_with_destination();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations/detail/reveal?name=platform-dev&key=cloudflare_token")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reveal_endpoint_rejects_unauthenticated_callers() {
    let (state, _sessions) = state_with_destination();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations/detail/reveal?name=platform-dev&key=cloudflare_token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::OK,
        "unauthenticated reveal must not succeed"
    );
}

#[tokio::test]
async fn non_admin_detail_view_shows_a_mask_instead_of_the_value() {
    let (state, sessions) = state_with_destination();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations/detail?name=platform-dev")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_of(response).await;

    assert!(html.contains("••••••••"), "expected a masked value for a non-admin");
    assert!(html.contains("aws_secret_access_key"), "key name should still show");
    assert!(!html.contains("revealed-aws_secret_access_key"));
}
