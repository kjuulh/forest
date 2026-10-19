use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const CID: &str = "forest_oa_datactl";
const SCOPES: &str = "datagateway:apply datagateway:read";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

fn device_app() -> Arc<MockOAuthAppsClient> {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_public_app(
        CID,
        vec!["http://127.0.0.1/callback".into()],
        vec!["datagateway:apply".into(), "datagateway:read".into()],
    );
    oauth.allow_device_grant(CID);
    oauth
}

fn app_with(
    oauth: Arc<MockOAuthAppsClient>,
) -> (
    axum::Router,
    Arc<forage_core::session::InMemorySessionStore>,
) {
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    (
        build_router(state.with_service_account_key("test-service-key".into())),
        sessions,
    )
}

fn form(uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

fn with_cookie(uri: &str, cookie: &str, body: Option<String>) -> Request<Body> {
    let builder = Request::builder().uri(uri).header("cookie", cookie);
    match body {
        Some(body) => builder
            .method("POST")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

fn start_body() -> String {
    format!("client_id={CID}&scope={}", urlencoding::encode(SCOPES))
}

fn poll_body(device_code: &str) -> String {
    format!(
        "grant_type={}&client_id={CID}&device_code={device_code}",
        urlencoding::encode(DEVICE_GRANT)
    )
}

#[tokio::test]
async fn a_cli_starts_a_grant_and_polls_until_approved() {
    let oauth = device_app();
    let (app, sessions) = app_with(oauth.clone());

    let started = app
        .clone()
        .oneshot(form("/oauth/device_authorization", start_body()))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let started = json(started).await;
    let device_code = started["device_code"].as_str().unwrap().to_string();
    let user_code = started["user_code"].as_str().unwrap().to_string();
    assert!(
        started["verification_uri"]
            .as_str()
            .unwrap()
            .ends_with("/device")
    );
    assert!(started["verification_uri_complete"].as_str().is_some());
    assert_eq!(started["expires_in"], 900);
    assert_eq!(started["interval"], 5);

    let pending = app
        .clone()
        .oneshot(form("/oauth/token", poll_body(&device_code)))
        .await
        .unwrap();
    assert_eq!(pending.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(pending).await["error"], "authorization_pending");

    let cookie = create_test_session(&sessions).await;
    let shown = app
        .clone()
        .oneshot(with_cookie(
            &format!("/device?user_code={user_code}"),
            &cookie,
            None,
        ))
        .await
        .unwrap();
    let page = text(shown).await;
    assert!(page.contains("Authorise Seeded App"), "{page}");
    assert!(page.contains("datagateway:apply"));
    assert!(page.contains(&format!("name=\"client_id\" value=\"{CID}\"")));
    assert!(
        !page.contains(&device_code),
        "the device_code never reaches a browser"
    );

    let approved = app
        .clone()
        .oneshot(with_cookie(
            "/device",
            &cookie,
            Some(format!(
                "_csrf=test-csrf&user_code={user_code}&action=approve&client_id={CID}"
            )),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    assert!(text(approved).await.contains("is authorised"));

    let issued = app
        .clone()
        .oneshot(form("/oauth/token", poll_body(&device_code)))
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued = json(issued).await;
    assert_eq!(issued["token_type"], "bearer");
    assert_eq!(issued["scope"], SCOPES);
    assert!(
        issued["access_token"]
            .as_str()
            .unwrap()
            .starts_with("forest_oat_")
    );
    assert!(issued["refresh_token"].as_str().is_some());
    assert!(issued.get("id_token").is_none());

    let replay = app
        .oneshot(form("/oauth/token", poll_body(&device_code)))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(replay).await["error"], "expired_token");
}

#[tokio::test]
async fn a_code_for_an_app_the_page_did_not_show_is_shown_not_approved() {
    let oauth = device_app();
    let (app, sessions) = app_with(oauth.clone());
    let started = json(
        app.clone()
            .oneshot(form("/oauth/device_authorization", start_body()))
            .await
            .unwrap(),
    )
    .await;
    let user_code = started["user_code"].as_str().unwrap();
    let cookie = create_test_session(&sessions).await;

    let typed = app
        .oneshot(with_cookie(
            "/device",
            &cookie,
            Some(format!(
                "_csrf=test-csrf&user_code={user_code}&action=approve"
            )),
        ))
        .await
        .unwrap();
    assert_eq!(typed.status(), StatusCode::OK);
    let page = text(typed).await;
    assert!(page.contains("Check what this code is for"), "{page}");
    assert!(page.contains("datagateway:read"));
    assert_eq!(
        oauth.device_grants()[0].state,
        MockDeviceState::Pending,
        "nothing is approved without the person seeing the app"
    );
}

#[tokio::test]
async fn denying_at_the_page_is_access_denied_for_the_cli() {
    let oauth = device_app();
    let (app, sessions) = app_with(oauth.clone());
    let started = json(
        app.clone()
            .oneshot(form("/oauth/device_authorization", start_body()))
            .await
            .unwrap(),
    )
    .await;
    let cookie = create_test_session(&sessions).await;
    app.clone()
        .oneshot(with_cookie(
            "/device",
            &cookie,
            Some(format!(
                "_csrf=test-csrf&user_code={}&action=deny&client_id={CID}",
                started["user_code"].as_str().unwrap()
            )),
        ))
        .await
        .unwrap();

    let denied = app
        .oneshot(form(
            "/oauth/token",
            poll_body(started["device_code"].as_str().unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(denied).await["error"], "access_denied");
}

#[tokio::test]
async fn an_app_without_the_device_grant_is_unauthorized_client() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_public_app(CID, vec![], vec!["datagateway:apply".into()]);
    let (app, _) = app_with(oauth);
    let resp = app
        .oneshot(form(
            "/oauth/device_authorization",
            format!("client_id={CID}&scope=datagateway:apply"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(resp).await["error"], "unauthorized_client");
}

#[tokio::test]
async fn unknown_clients_and_unregistered_scopes_are_refused() {
    let (app, _) = app_with(device_app());

    let unknown = app
        .clone()
        .oneshot(form(
            "/oauth/device_authorization",
            "client_id=forest_oa_nobody&scope=datagateway:read".into(),
        ))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(unknown).await["error"], "invalid_client");

    let missing = app
        .clone()
        .oneshot(form(
            "/oauth/device_authorization",
            "scope=datagateway:read".into(),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let scope = app
        .clone()
        .oneshot(form(
            "/oauth/device_authorization",
            format!("client_id={CID}&scope=profile"),
        ))
        .await
        .unwrap();
    assert_eq!(scope.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(scope).await["error"], "invalid_scope");

    let no_code = app
        .oneshot(form(
            "/oauth/token",
            format!(
                "grant_type={}&client_id={CID}",
                urlencoding::encode(DEVICE_GRANT)
            ),
        ))
        .await
        .unwrap();
    assert_eq!(no_code.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(no_code).await["error"], "invalid_request");
}

#[tokio::test]
async fn discovery_advertises_the_device_endpoint_and_grant() {
    let (app, _) = app_with(device_app());
    let doc = json(
        app.oneshot(
            Request::builder()
                .uri("/.well-known/openid-configuration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        doc["device_authorization_endpoint"]
            .as_str()
            .unwrap()
            .ends_with("/oauth/device_authorization")
    );
    assert!(
        doc["grant_types_supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g == DEVICE_GRANT)
    );
}

#[tokio::test]
async fn a_forest_cli_code_still_renders_the_forest_cli_page() {
    let (app, sessions) = app_with(device_app());
    let cookie = create_test_session(&sessions).await;
    let page = text(
        app.oneshot(with_cookie("/device?user_code=ZZZZ-ZZZZ", &cookie, None))
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains("Authorise Forest CLI"));
    assert!(!page.contains("name=\"client_id\""));
}
