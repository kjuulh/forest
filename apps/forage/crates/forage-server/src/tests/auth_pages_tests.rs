use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use forage_core::auth::magic_link::{
    InMemoryMagicLinkStore, MagicLinkStore, TOKEN_TYPE_EMAIL_VERIFY, generate_magic_link_token,
};
use forage_core::auth::{LoginResult, RegisterResult};
use tower::ServiceExt;

use crate::state::AppState;
use crate::test_support::*;

struct Page {
    status: StatusCode,
    headers: HeaderMap,
    html: String,
}

async fn request(state: &AppState, cookie: &str, method: &str, uri: &str, form: &str) -> Page {
    let mut builder = Request::builder().method(method).uri(uri);
    if !cookie.is_empty() {
        builder = builder.header("cookie", cookie);
    }
    if method == "POST" {
        builder = builder.header("content-type", "application/x-www-form-urlencoded");
    }
    let response = crate::build_router(state.clone())
        .oneshot(builder.body(Body::from(form.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Page {
        status,
        headers,
        html: String::from_utf8(body.to_vec()).unwrap(),
    }
}

async fn get(state: &AppState, uri: &str) -> Page {
    request(state, "", "GET", uri, "").await
}

fn main_element(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    html[start..end].to_string()
}

fn masked_main(html: &str) -> String {
    let main = main_element(html);
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&main, "?v=HASH");
    regex::Regex::new(r#"name="consent_binding" value="[^"]*""#)
        .unwrap()
        .replace_all(&main, r#"name="consent_binding" value="BINDING""#)
        .into_owned()
}

fn nav(html: &str) -> String {
    let start = html.find("<nav").unwrap();
    let end = html.find("</nav>").unwrap() + "</nav>".len();
    html[start..end].to_string()
}

fn assert_clean(html: &str) {
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    assert!(!html.contains("<style"), "no inline style elements: {html}");
    assert!(!html.contains("style="), "no inline styles: {html}");
    for handler in ["onsubmit", "onclick", "onchange", "onload"] {
        assert!(!html.contains(handler), "no {handler}: {html}");
    }
    let main = main_element(html);
    assert!(!main.contains(">none<"), "no none leak: {main}");
    assert!(!main.contains("value=\"none\""), "no none leak: {main}");
    assert!(!main.contains("/none"), "no none leak: {main}");
}

fn assert_visitor_auth_nav(html: &str) {
    let nav = nav(html);
    assert!(nav.contains(r#"href="/login""#), "{nav}");
    assert!(!nav.contains("/pricing"), "marketing links hidden: {nav}");
    assert!(
        !nav.contains("/components"),
        "marketing links hidden: {nav}"
    );
    assert!(!nav.contains("/logout"), "no signed-in chrome: {nav}");
}

#[derive(Debug, PartialEq)]
struct Field {
    tag: String,
    name: String,
    kind: String,
    value: Option<String>,
}

#[derive(Debug)]
struct Form {
    action: String,
    method: String,
    fields: Vec<Field>,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    regex::Regex::new(&format!(r#"\s{name}="([^"]*)""#))
        .unwrap()
        .captures(tag)
        .map(|c| c[1].to_string())
}

fn forms(html: &str) -> Vec<Form> {
    let main = main_element(html);
    let form_re = regex::Regex::new(r#"(?s)(<form[^>]*>)(.*?)</form>"#).unwrap();
    let field_re =
        regex::Regex::new(r#"(?s)<(input|textarea|select|button)\b[^>]*\sname="[^"]+"[^>]*>"#)
            .unwrap();
    form_re
        .captures_iter(&main)
        .map(|c| Form {
            action: attr(&c[1], "action").unwrap_or_default(),
            method: attr(&c[1], "method").unwrap_or_default(),
            fields: field_re
                .captures_iter(&c[2])
                .map(|f| Field {
                    tag: f[0].to_string(),
                    name: attr(&f[0], "name").unwrap(),
                    kind: attr(&f[0], "type").unwrap_or_else(|| f[1].to_string()),
                    value: attr(&f[0], "value"),
                })
                .collect(),
        })
        .collect()
}

fn only_form(html: &str) -> Form {
    let mut found = forms(html);
    assert_eq!(found.len(), 1, "one form: {found:?}");
    found.remove(0)
}

fn names(form: &Form) -> Vec<&str> {
    form.fields.iter().map(|f| f.name.as_str()).collect()
}

fn field<'a>(form: &'a Form, name: &str) -> &'a Field {
    form.fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}: {form:?}"))
}

fn hidden(form: &Form, name: &str) -> String {
    let f = field(form, name);
    assert_eq!(f.kind, "hidden", "{f:?}");
    f.value.clone().unwrap_or_default()
}

fn with_providers(state: AppState) -> AppState {
    state
        .with_github_oauth_config(crate::state::GitHubOAuthConfig {
            client_id: "gh".into(),
            client_secret: "secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "g".into(),
            client_secret: "secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_magic_link_store(Arc::new(InMemoryMagicLinkStore::new()))
}

const RETURN_TO: &str = "%2Fdevice%3Fuser_code%3DABCD-EFGH";

#[tokio::test]
async fn login_form_keeps_its_action_fields_and_return_to() {
    let (state, _) = test_state();
    let state = with_providers(state);

    let page = get(&state, &format!("/login?return_to={RETURN_TO}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert_visitor_auth_nav(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/login", "POST")
    );
    assert_eq!(
        names(&form),
        ["return_to", "identifier", "password", "remember_me"]
    );
    assert_eq!(hidden(&form, "return_to"), "/device?user_code=ABCD-EFGH");
    assert_eq!(field(&form, "identifier").value.as_deref(), Some(""));
    assert_eq!(field(&form, "password").kind, "password");
    assert_eq!(field(&form, "password").value, None);
    let remember = field(&form, "remember_me");
    assert_eq!(remember.kind, "checkbox");
    assert_eq!(remember.value.as_deref(), Some("on"));
    assert!(remember.tag.contains(" checked"), "{}", remember.tag);

    let encoded = "?return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH";
    for path in [
        "/auth/google",
        "/auth/github",
        "/auth/magic-link",
        "/signup",
    ] {
        assert!(
            page.html.contains(&format!(r#"href="{path}{encoded}""#)),
            "{path} carries return_to"
        );
    }
    assert!(page.html.contains("Sign in with email link"));
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn login_without_return_to_or_providers_has_no_hidden_field_or_divider() {
    let (state, _) = test_state();
    let page = get(&state, "/login").await;
    let form = only_form(&page.html);
    assert_eq!(names(&form), ["identifier", "password", "remember_me"]);
    let main = main_element(&page.html);
    assert!(!main.contains("return_to"), "{main}");
    assert!(!main.contains(">or<"), "{main}");
    assert!(main.contains(r#"href="/signup""#), "{main}");
}

#[tokio::test]
async fn unsafe_return_to_is_not_rendered_anywhere() {
    let (state, _) = test_state();
    let state = with_providers(state);
    for raw in ["%2F%2Fevil.com", "https%3A%2F%2Fevil.com", "%2F%5Cevil.com"] {
        for path in ["/login", "/signup"] {
            let page = get(&state, &format!("{path}?return_to={raw}")).await;
            assert_eq!(page.status, StatusCode::OK);
            let main = main_element(&page.html);
            assert!(!main.contains("return_to"), "{path} {raw}: {main}");
            assert!(!main.contains("evil.com"), "{path} {raw}: {main}");
        }
    }
}

#[tokio::test]
async fn login_error_re_renders_with_identifier_escaped() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        login_result: Some(Err(forage_core::auth::AuthError::InvalidCredentials)),
        ..Default::default()
    });
    let (state, _) = test_state_with(mock, MockPlatformClient::new());
    let page = request(
        &state,
        "",
        "POST",
        "/login",
        "identifier=%22%3E%3Cb%3Ex&password=wrong&return_to=%2Fdashboard",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    let form = only_form(&page.html);
    assert_eq!(hidden(&form, "return_to"), "/dashboard");
    assert!(
        page.html.contains(r#"value="&quot;&gt;&lt;b&gt;x""#),
        "{}",
        page.html
    );
    assert!(page.html.contains("Invalid email/username or password"));
}

#[tokio::test]
async fn signup_form_keeps_its_action_fields_and_return_to() {
    let (state, _) = test_state();
    let state = with_providers(state);

    let page = get(&state, &format!("/signup?return_to={RETURN_TO}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert_visitor_auth_nav(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/signup", "POST")
    );
    assert_eq!(
        names(&form),
        [
            "return_to",
            "username",
            "email",
            "password",
            "password_confirm"
        ]
    );
    assert_eq!(hidden(&form, "return_to"), "/device?user_code=ABCD-EFGH");
    assert_eq!(field(&form, "password").kind, "password");
    assert_eq!(field(&form, "password_confirm").kind, "password");
    assert!(page.html.contains("Sign up with email link"));
    assert!(
        page.html
            .contains(r#"href="/login?return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH""#)
    );
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn signup_error_keeps_username_and_email_but_never_the_password() {
    let (state, _) = test_state();
    let page = request(
        &state,
        "",
        "POST",
        "/signup",
        "username=alice&email=alice%40example.com&password=SecurePass123&password_confirm=Other12345678",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    let form = only_form(&page.html);
    assert_eq!(field(&form, "username").value.as_deref(), Some("alice"));
    assert_eq!(
        field(&form, "email").value.as_deref(),
        Some("alice@example.com")
    );
    assert!(!page.html.contains("SecurePass123"));
    assert!(page.html.contains("Passwords do not match"));
}

#[tokio::test]
async fn mfa_challenge_keeps_the_code_field_return_to_and_cookie() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        login_result: Some(Ok(LoginResult::MfaRequired {
            mfa_session_token: "mfa-tok".into(),
        })),
        ..Default::default()
    });
    let (state, _) = test_state_with(mock, MockPlatformClient::new());
    let page = request(
        &state,
        "",
        "POST",
        "/login",
        &format!("identifier=alice&password=pw&return_to={RETURN_TO}"),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(
        page.headers.get("set-cookie").unwrap(),
        "forage_mfa_session=mfa-tok; HttpOnly; SameSite=Lax; Path=/login; Max-Age=300"
    );
    assert_clean(&page.html);
    assert_visitor_auth_nav(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/login/mfa", "POST")
    );
    assert_eq!(names(&form), ["return_to", "code"]);
    assert_eq!(hidden(&form, "return_to"), "/device?user_code=ABCD-EFGH");
    let code = &field(&form, "code").tag;
    for a in [
        r#"autocomplete="one-time-code""#,
        r#"inputmode="numeric""#,
        r#"pattern="[0-9]{6}""#,
        r#"maxlength="6""#,
        " autofocus",
        " required",
    ] {
        assert!(code.contains(a), "{a} in {code}");
    }
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn mfa_challenge_failure_shows_the_error_and_keeps_return_to() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        verify_login_mfa_result: Some(Err(forage_core::auth::AuthError::InvalidCredentials)),
        ..Default::default()
    });
    let (state, _) = test_state_with(mock, MockPlatformClient::new());
    let page = request(
        &state,
        "forage_mfa_session=mfa-tok",
        "POST",
        "/login/mfa",
        "code=000000&return_to=%2Fsettings%2Faccount",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    let form = only_form(&page.html);
    assert_eq!(hidden(&form, "return_to"), "/settings/account");
    assert!(
        page.html
            .contains("Invalid or expired code. Please try again.")
    );
}

#[tokio::test]
async fn complete_profile_form_keeps_csrf_return_to_and_username() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_needs_username(&sessions).await;
    let page = request(
        &state,
        &cookie,
        "GET",
        "/auth/complete-profile?return_to=%2Fdashboard",
        "",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert_visitor_auth_nav(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/auth/complete-profile", "POST")
    );
    assert_eq!(names(&form), ["_csrf", "return_to", "username"]);
    assert_eq!(hidden(&form, "_csrf"), "test-csrf");
    assert_eq!(hidden(&form, "return_to"), "/dashboard");
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn magic_link_form_and_sent_page() {
    let (state, _) = test_state_with_magic_link();
    let page = get(&state, "/auth/magic-link").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert_visitor_auth_nav(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/auth/magic-link", "POST")
    );
    assert_eq!(names(&form), ["email"]);
    assert_eq!(field(&form, "email").kind, "email");
    insta::assert_snapshot!("magic_link_form", masked_main(&page.html));

    let invalid = request(&state, "", "POST", "/auth/magic-link", "email=nope").await;
    let form = only_form(&invalid.html);
    assert_eq!(field(&form, "email").value.as_deref(), Some("nope"));
    assert!(main_element(&invalid.html).contains("bg-red-50"));

    let sent = request(
        &state,
        "",
        "POST",
        "/auth/magic-link",
        "email=alice%40example.com",
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_clean(&sent.html);
    assert_visitor_auth_nav(&sent.html);
    assert!(forms(&sent.html).is_empty());
    assert!(sent.html.contains("<strong>alice@example.com</strong>"));
    insta::assert_snapshot!("magic_link_sent", masked_main(&sent.html));
}

#[tokio::test]
async fn verify_email_pages_render_with_their_status_and_headers() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        register_result: Some(Ok(RegisterResult::VerificationRequired)),
        ..Default::default()
    });
    let (state, _) = test_state_with(mock, MockPlatformClient::new());
    let store = Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());

    let inbox = request(
        &state,
        "",
        "POST",
        "/signup",
        "username=alice&email=alice%40example.com&password=SecurePass123&password_confirm=SecurePass123",
    )
    .await;
    assert_eq!(inbox.status, StatusCode::OK);
    assert_clean(&inbox.html);
    assert_visitor_auth_nav(&inbox.html);
    assert!(inbox.html.contains("<strong>alice@example.com</strong>"));
    insta::assert_snapshot!("verify_email_check_inbox", masked_main(&inbox.html));

    let failed = get(&state, "/auth/verify-email?token=bogus").await;
    assert_eq!(failed.status, StatusCode::BAD_REQUEST);
    assert_clean(&failed.html);
    assert_visitor_auth_nav(&failed.html);
    insta::assert_snapshot!("verify_email_failed", masked_main(&failed.html));

    let resend = get(&state, "/auth/verify-email/resend").await;
    assert_eq!(resend.status, StatusCode::OK);
    assert_clean(&resend.html);
    let form = only_form(&resend.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/auth/verify-email/resend", "post")
    );
    assert_eq!(names(&form), ["email"]);
    assert_eq!(field(&form, "email").value.as_deref(), Some(""));
    insta::assert_snapshot!("verify_email_resend", masked_main(&resend.html));

    let (raw, hash) = generate_magic_link_token();
    store
        .store_token(
            TOKEN_TYPE_EMAIL_VERIFY,
            &hash,
            "alice@example.com",
            chrono::Utc::now() + chrono::Duration::minutes(15),
            None,
        )
        .await
        .unwrap();
    let verified = get(
        &state,
        &format!("/auth/verify-email?token={}", urlencoding::encode(&raw)),
    )
    .await;
    assert_eq!(verified.status, StatusCode::OK);
    assert_eq!(
        verified.headers.get("referrer-policy").unwrap(),
        "no-referrer"
    );
    assert_clean(&verified.html);
    assert_visitor_auth_nav(&verified.html);
    insta::assert_snapshot!("verify_email_success", masked_main(&verified.html));
}

#[tokio::test]
async fn device_page_keeps_csrf_code_and_actions_and_the_signed_in_chrome() {
    let (state, sessions) = test_state();
    let state = state.with_service_account_key("key".into());
    let cookie = create_test_session(&sessions).await;
    let page = request(&state, &cookie, "GET", "/device?user_code=ABCD-EFGH", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/device", "POST")
    );
    assert_eq!(names(&form), ["_csrf", "user_code", "action", "action"]);
    assert_eq!(hidden(&form, "_csrf"), "test-csrf");
    let code = field(&form, "user_code");
    assert_eq!(code.value.as_deref(), Some("ABCD-EFGH"));
    assert!(code.tag.contains(r#"autocomplete="off""#), "{}", code.tag);
    assert!(code.tag.contains(r#"spellcheck="false""#), "{}", code.tag);
    let actions: Vec<_> = form
        .fields
        .iter()
        .filter(|f| f.name == "action")
        .map(|f| f.value.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(actions, ["approve", "deny"]);

    let nav = nav(&page.html);
    assert!(
        nav.contains(r#"action="/logout""#),
        "signed-in chrome: {nav}"
    );
    assert!(nav.contains(r#"href="/orgs/testorg/projects""#), "{nav}");
    assert!(page.html.contains("<strong>testuser</strong>"));
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn device_outcomes_render_without_a_form() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let unconfigured = request(&state, &cookie, "GET", "/device", "").await;
    assert!(forms(&unconfigured.html).is_empty());
    assert!(unconfigured.html.contains("FOREST_SERVICE_ACCOUNT_API_KEY"));

    let state = state.with_service_account_key("key".into());
    let approved = request(
        &state,
        &cookie,
        "POST",
        "/device",
        "_csrf=test-csrf&user_code=ABCD-EFGH&action=approve",
    )
    .await;
    assert!(forms(&approved.html).is_empty());
    assert!(approved.html.contains("You're signed in."));
    insta::assert_snapshot!("device_approved", masked_main(&approved.html));

    let denied = request(
        &state,
        &cookie,
        "POST",
        "/device",
        "_csrf=test-csrf&user_code=ABCD-EFGH&action=deny",
    )
    .await;
    assert!(forms(&denied.html).is_empty());
    assert!(denied.html.contains("Sign-in denied."));
}

#[tokio::test]
async fn device_page_for_an_app_carries_its_client_id_and_scopes() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_public_app(
        "forest_oa_cli",
        vec!["http://127.0.0.1/callback".into()],
        vec!["datagateway:apply".into(), "datagateway:read".into()],
    );
    oauth.allow_device_grant("forest_oa_cli");
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let state = state.with_service_account_key("key".into());
    let started = request(
        &state,
        "",
        "POST",
        "/oauth/device_authorization",
        "client_id=forest_oa_cli&scope=datagateway%3Aapply+datagateway%3Aread",
    )
    .await;
    let started: serde_json::Value = serde_json::from_str(&started.html).unwrap();
    let user_code = started["user_code"].as_str().unwrap().to_string();
    let cookie = create_test_session(&sessions).await;

    let page = request(
        &state,
        &cookie,
        "GET",
        &format!("/device?user_code={user_code}"),
        "",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        names(&form),
        ["_csrf", "client_id", "user_code", "action", "action"]
    );
    assert_eq!(hidden(&form, "client_id"), "forest_oa_cli");
    assert!(page.html.contains("Authorise Seeded App"));
    assert!(page.html.contains(
        r#"<span class="font-mono">datagateway:apply</span> — Define data gateway project metrics"#
    ));
    insta::assert_snapshot!(masked_main(&page.html).replace(&user_code, "USER-CODE"));
}

fn consent_state() -> (AppState, Arc<forage_core::session::InMemorySessionStore>) {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_app(
        "forest_oa_testclient",
        vec!["https://app.example/cb".into()],
        vec!["openid".into(), "profile".into(), "email".into()],
    );
    test_state_with_oauth_apps(oauth)
}

#[tokio::test]
async fn oauth_consent_form_keeps_every_oauth_parameter() {
    let (state, sessions) = consent_state();
    let cookie = create_test_session(&sessions).await;
    let uri = "/oauth/authorize?client_id=forest_oa_testclient\
        &redirect_uri=https%3A%2F%2Fapp.example%2Fcb&response_type=code\
        &scope=openid+profile&state=st%22%3E%3Cx&code_challenge=chal\
        &code_challenge_method=S256&nonce=n-1";
    let page = request(&state, &cookie, "GET", uri, "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    let form = only_form(&page.html);
    assert_eq!(
        (form.action.as_str(), form.method.as_str()),
        ("/oauth/authorize", "POST")
    );
    assert_eq!(
        names(&form),
        [
            "_csrf",
            "client_id",
            "redirect_uri",
            "scope",
            "consent_binding",
            "state",
            "code_challenge",
            "code_challenge_method",
            "nonce",
            "action",
            "action",
        ]
    );
    assert_eq!(hidden(&form, "_csrf"), "test-csrf");
    assert_eq!(hidden(&form, "client_id"), "forest_oa_testclient");
    assert_eq!(hidden(&form, "redirect_uri"), "https://app.example/cb");
    assert_eq!(hidden(&form, "scope"), "openid profile");
    assert_eq!(
        hidden(&form, "consent_binding"),
        crate::routes::oauth::consent_binding(
            "test-csrf",
            "forest_oa_testclient",
            "https://app.example/cb",
            "openid profile",
        )
    );
    assert_eq!(hidden(&form, "state"), "st&quot;&gt;&lt;x");
    assert_eq!(hidden(&form, "code_challenge"), "chal");
    assert_eq!(hidden(&form, "code_challenge_method"), "S256");
    assert_eq!(hidden(&form, "nonce"), "n-1");
    let nav = nav(&page.html);
    assert!(
        nav.contains(r#"action="/logout""#),
        "signed-in chrome: {nav}"
    );
    assert!(
        page.html
            .contains(r#"(<span class="font-mono">testuser</span>)"#)
    );
    assert!(page.html.contains("Confirm your identity"));
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn oauth_consent_renders_absent_parameters_as_empty_never_none() {
    let (state, sessions) = consent_state();
    let cookie = create_test_session(&sessions).await;
    let uri = "/oauth/authorize?client_id=forest_oa_testclient\
        &redirect_uri=https%3A%2F%2Fapp.example%2Fcb&response_type=code&scope=profile";
    let page = request(&state, &cookie, "GET", uri, "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    let form = only_form(&page.html);
    for name in ["state", "code_challenge", "code_challenge_method", "nonce"] {
        assert_eq!(hidden(&form, name), "", "{name}");
        assert!(
            form.fields
                .iter()
                .any(|f| f.name == name && f.tag.contains(r#"value="""#)),
            "{name} renders value=\"\""
        );
    }
}

#[tokio::test]
async fn the_magic_link_form_carries_a_safe_return_to_and_drops_an_unsafe_one() {
    let (state, _) = test_state_with_magic_link();
    let page = get(&state, "/auth/magic-link?return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH").await;
    let form = only_form(&page.html);
    assert_eq!(names(&form), ["return_to", "email"]);
    assert_eq!(field(&form, "return_to").value.as_deref(), Some("/device?user_code=ABCD-EFGH"));

    let page = get(&state, "/auth/magic-link?return_to=%2F%2Fevil.example").await;
    assert_eq!(names(&only_form(&page.html)), ["email"]);
}
