use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde_json::{Value, json};

/// Test-only RSA key for the fake GitHub App; it signs nothing real.
pub(crate) const TEST_APP_KEY: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIICXAIBAAKBgQDBnIB1DEBxG4fPVZzekrqOfATms2DFcbg0NG3YVh8uastZsT7P
RlMzhWjhMgR0w+XbxUXK6IPTk/t+w8qd5pjmArh9zeHdxhUYH9l++gJFkFGiQkzL
tXaHGGT8TqI0CQtoILjbOJToUc53BCxodoAAEqaCemI/XvBscOZ38VRZOQIDAQAB
AoGAInhOlog74OdZ4pwHdckB1eRWqww8qmp4tTKwa9fJbWYuFiXsXNLUdVPHPHC/
QlquTdZtepazAbRTDlqldD9d8SYkbY+tJZyzAzjc59EA4N2PAIPq4A2+2PQBnh3H
aMr1rn9P88V33iSIcfmXDgRBI3rfQoaQ7guS1bN7+F6wDAECQQDpx38tLICzzqk/
Oe2hYPVpUgILskF9fZIKjZDDjw5S+RbCM8Ajus3Z4QUQaJSEYwqv9cVEl7ZQMNin
z1hO5gKZAkEA1AObR5I7ETVSRRt40aNKZPYDq7DU+kzivLGMYRdkJTYBOKQbDN+k
Ec3iPf0OleT4+8YfSYymclaWMSOASgvPoQJBALFBJSmAU7mB2YvqBUZWON15Y7mw
l71pp4C+4+OTJV4+JUZsbKPqsMghuHA/i1RxsBnItKEUkxRsZOmf6iowdgkCQDZ7
/smz0K4pb80GtFPbjODJQEQ9G15/p70qO5Mv+bJ+YG337mOujXFt4XhgsgDfaQT6
80tpc9TqX4qdPvjIdEECQD9UcykvzZRAIEc6XFyrhobv1gciHSOcEuUz/jlTvaPG
6KERBTkkh4KqnsP+pZ9A5ZLaYKyA6YqN9/brfZzMgjQ=
-----END RSA PRIVATE KEY-----"#;

#[derive(Default)]
pub(crate) struct ProviderState {
    pub oauth_access: Value,
    pub slack_users: HashMap<String, Value>,
    pub slack_lookup_error: Option<String>,
    pub post_error: Option<String>,
    pub posted: Vec<(String, String)>,
    pub updated: Vec<(String, String)>,
    pub lookups: Vec<String>,
    pub gh_user: Value,
    pub gh_user_installations: Vec<i64>,
    pub gh_installations: HashMap<i64, Value>,
    pub gh_memberships: HashMap<(String, String), Value>,
    pub gh_members: Vec<i64>,
}

#[derive(Clone)]
pub(crate) struct FakeProviders {
    pub base: String,
    pub state: Arc<Mutex<ProviderState>>,
}

impl FakeProviders {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(ProviderState::default()));
        let app = Router::new()
            .route("/slack/api/oauth.v2.access", post(oauth_access))
            .route("/slack/api/users.lookupByEmail", get(lookup_by_email))
            .route("/slack/api/chat.postMessage", post(post_message))
            .route("/slack/api/chat.update", post(update_message))
            .route(
                "/slack/api/conversations.join",
                post(|| async { Json(json!({"ok": true})) }),
            )
            .route("/github/login/oauth/access_token", post(gh_token))
            .route("/github/api/user", get(gh_user))
            .route("/github/api/user/installations", get(gh_user_installations))
            .route("/github/api/app/installations/{id}", get(gh_installation))
            .route(
                "/github/api/app/installations/{id}/access_tokens",
                post(gh_access_token),
            )
            .route(
                "/github/api/orgs/{org}/memberships/{login}",
                get(gh_membership),
            )
            .route("/github/api/orgs/{org}/members", get(gh_members))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base: format!("http://{addr}"),
            state,
        }
    }

    pub fn slack_api(&self) -> crate::providers::SlackApi {
        crate::providers::SlackApi::new(format!("{}/slack/api", self.base))
    }

    pub fn github_app(&self, webhook_secret: &str) -> crate::providers::GitHubApp {
        crate::providers::GitHubApp::new(crate::providers::GitHubAppConfig {
            app_id: "123".into(),
            slug: "forest-test".into(),
            client_id: "Iv1.test".into(),
            client_secret: "test-secret".into(),
            webhook_secret: webhook_secret.into(),
            private_key_pem: TEST_APP_KEY.into(),
            api_base: format!("{}/github/api", self.base),
            web_base: format!("{}/github", self.base),
        })
        .unwrap()
    }

    pub fn add_slack_user(&self, email: &str, id: &str, team: &str) {
        self.state.lock().unwrap().slack_users.insert(
            email.into(),
            json!({ "id": id, "team_id": team, "deleted": false, "is_bot": false,
                    "is_email_confirmed": true, "profile": { "email": email } }),
        );
    }

    pub fn dm_recipients(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .posted
            .iter()
            .map(|(c, _)| c.clone())
            .collect()
    }
}

type S = State<Arc<Mutex<ProviderState>>>;

async fn oauth_access(State(s): S) -> Json<Value> {
    Json(s.lock().unwrap().oauth_access.clone())
}

async fn lookup_by_email(State(s): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let mut st = s.lock().unwrap();
    let email = q.get("email").cloned().unwrap_or_default();
    st.lookups.push(email.clone());
    if let Some(err) = &st.slack_lookup_error {
        return Json(json!({ "ok": false, "error": err }));
    }
    match st.slack_users.get(&email) {
        Some(u) => Json(json!({ "ok": true, "user": u })),
        None => Json(json!({ "ok": false, "error": "users_not_found" })),
    }
}

async fn post_message(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let mut st = s.lock().unwrap();
    if let Some(err) = &st.post_error {
        return Json(json!({ "ok": false, "error": err }));
    }
    let channel = body["channel"].as_str().unwrap_or_default().to_string();
    st.posted.push((
        channel.clone(),
        body["text"].as_str().unwrap_or_default().to_string(),
    ));
    let ts = format!("1700000000.{:06}", st.posted.len());
    Json(json!({ "ok": true, "channel": channel, "ts": ts }))
}

async fn update_message(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let mut st = s.lock().unwrap();
    st.updated.push((
        body["channel"].as_str().unwrap_or_default().to_string(),
        body["ts"].as_str().unwrap_or_default().to_string(),
    ));
    Json(json!({ "ok": true }))
}

async fn gh_token(Form(form): Form<HashMap<String, String>>) -> Json<Value> {
    if form.get("code").map(String::as_str) == Some("good-code") {
        Json(json!({ "access_token": "gho_user" }))
    } else {
        Json(json!({ "error": "bad_verification_code" }))
    }
}

async fn gh_user(State(s): S) -> Json<Value> {
    Json(s.lock().unwrap().gh_user.clone())
}

async fn gh_user_installations(State(s): S) -> Json<Value> {
    let ids = s.lock().unwrap().gh_user_installations.clone();
    Json(json!({ "installations": ids.iter().map(|i| json!({"id": i})).collect::<Vec<_>>() }))
}

async fn gh_installation(State(s): S, Path(id): Path<i64>) -> axum::response::Response {
    use axum::response::IntoResponse;
    match s.lock().unwrap().gh_installations.get(&id) {
        Some(v) => Json(v.clone()).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn gh_access_token() -> Json<Value> {
    Json(json!({ "token": "ghs_installation" }))
}

async fn gh_membership(
    State(s): S,
    Path((org, login)): Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match s.lock().unwrap().gh_memberships.get(&(org, login)) {
        Some(v) => Json(v.clone()).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn gh_members(State(s): S) -> Json<Value> {
    let ids = s.lock().unwrap().gh_members.clone();
    Json(json!(
        ids.iter()
            .map(|i| json!({"id": i, "login": format!("u{i}")}))
            .collect::<Vec<_>>()
    ))
}
