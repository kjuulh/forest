//! Real-socket end-to-end test for the public OAuth flow (M5 / item C).
//!
//! Unlike the `tower::oneshot` route tests, this binds the actual Forage router
//! to a real TCP port and drives it with `reqwest` — exercising real HTTP
//! transport, form-encoding, 3xx handling, and JSON deserialization, the way an
//! off-the-shelf OAuth client would. The forest-server side is mocked (its gRPC
//! wire is covered by forest's accept tests), so this isolates the HTTP/OAuth
//! contract Forage exposes.

use std::sync::Arc;

use tokio::net::TcpListener;

use crate::build_router;
use crate::test_support::*;

const CID: &str = "forest_oa_e2eclient";
const REDIRECT: &str = "https://app.example/cb";

fn code_from_location(loc: &str) -> Option<String> {
    let q = loc.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "code").then(|| v.to_string())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_http_client_completes_authorization_code_flow() {
    // Arrange: a seeded app, a logged-in session, the real router on a port.
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_app(
        CID,
        vec![REDIRECT.into()],
        vec!["profile".into(), "email".into()],
    );
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // 1. Consent approval (the browser step). A real client redirects the user
    //    here; we stand in for the authenticated browser via the session cookie.
    let binding = crate::routes::oauth::consent_binding("test-csrf", CID, REDIRECT, "profile email");
    let consent = http
        .post(format!("{base}/oauth/authorize"))
        .header("cookie", &cookie)
        .form(&[
            ("_csrf", "test-csrf"),
            ("action", "approve"),
            ("client_id", CID),
            ("redirect_uri", REDIRECT),
            ("scope", "profile email"),
            ("consent_binding", binding.as_str()),
            ("state", "xyz-123"),
            ("code_challenge", ""),
            ("code_challenge_method", ""),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(consent.status().as_u16(), 303, "consent → redirect with code");
    let loc = consent
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(loc.starts_with(REDIRECT));
    assert!(loc.contains("state=xyz-123"));
    let code = code_from_location(&loc).expect("authorization code in redirect");

    // 2. Token exchange — real form POST, real JSON parse (what a client does).
    let token_resp = http
        .post(format!("{base}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CID),
            ("client_secret", "the-secret"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
        ])
        .send()
        .await
        .unwrap();
    assert!(token_resp.status().is_success(), "token endpoint 2xx");
    let tokens: serde_json::Value = token_resp.json().await.unwrap();
    let access = tokens["access_token"].as_str().unwrap().to_string();
    assert!(access.starts_with("forest_oat_"));
    assert_eq!(tokens["token_type"], "bearer");
    assert!(tokens["expires_in"].as_i64().unwrap() > 0);
    assert_eq!(tokens["scope"], "profile email");

    // 3. Userinfo with the issued bearer token.
    let ui = http
        .get(format!("{base}/oauth/userinfo"))
        .bearer_auth(&access)
        .send()
        .await
        .unwrap();
    assert!(ui.status().is_success(), "userinfo 2xx with valid bearer");
    let claims: serde_json::Value = ui.json().await.unwrap();
    assert_eq!(claims["sub"], "user-1");

    // 4. Userinfo without a token → 401 with a Bearer challenge.
    let unauth = http
        .get(format!("{base}/oauth/userinfo"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status().as_u16(), 401);
    assert!(unauth.headers().get("www-authenticate").is_some());
}
