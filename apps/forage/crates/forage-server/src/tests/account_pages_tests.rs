use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::{LinkedIdentity, LinkedProvider, PersonalAccessToken, User};
use forage_core::platform::{
    Artifact, ArtifactContext, ArtifactRef, ArtifactSource, NotificationPreference,
};
use forage_core::session::InMemorySessionStore;
use tower::ServiceExt;

use crate::state::AppState;
use crate::test_support::*;
use crate::views::account::contribution_level;

struct Page {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    html: String,
}

async fn request(state: &AppState, cookie: &str, method: &str, uri: &str, form: &str) -> Page {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", cookie);
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

async fn session(state_and_sessions: (AppState, Arc<InMemorySessionStore>)) -> (AppState, String) {
    let (state, sessions) = state_and_sessions;
    let cookie = create_test_session(&sessions).await;
    (state, cookie)
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
    let main = regex::Regex::new(r"<svg xmlns=[^>]*>(?s:.*?)</svg>")
        .unwrap()
        .replace_all(&main, "<svg>QR</svg>");
    regex::Regex::new(
        r">\s*[0-9]+ (seconds?|minutes?|hours?|days?|months?|years?) ago\s*<|>\s*just now\s*<",
    )
    .unwrap()
    .replace_all(&main, ">AGO<")
    .into_owned()
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
    assert!(
        !main.contains("csrf_token"),
        "every form uses _csrf: {main}"
    );
}

struct Form {
    action: String,
    method: String,
    fields: Vec<String>,
    tag: String,
}

fn forms(html: &str) -> Vec<Form> {
    let main = main_element(html);
    let form_re = regex::Regex::new(r#"(?s)(<form[^>]*>)(.*?)</form>"#).unwrap();
    let attr = |tag: &str, name: &str| {
        regex::Regex::new(&format!(r#"\s{name}="([^"]*)""#))
            .unwrap()
            .captures(tag)
            .map(|c| c[1].to_string())
            .unwrap_or_default()
    };
    let field_re =
        regex::Regex::new(r#"<(?:input|textarea|select|button)[^>]*\sname="([^"]+)""#).unwrap();
    form_re
        .captures_iter(&main)
        .map(|c| Form {
            action: attr(&c[1], "action"),
            method: attr(&c[1], "method"),
            fields: field_re
                .captures_iter(&c[2])
                .map(|f| f[1].to_string())
                .collect(),
            tag: c[0].to_string(),
        })
        .collect()
}

fn assert_forms(html: &str, expected: &[(&str, &[&str])]) {
    let found = forms(html);
    for form in &found {
        assert_eq!(form.method, "POST", "{}", form.tag);
        assert!(
            form.tag
                .contains(r#"<input type="hidden" name="_csrf" value="test-csrf">"#),
            "form posts _csrf: {}",
            form.tag
        );
    }
    let actual: Vec<(String, Vec<String>)> = found
        .iter()
        .map(|f| (f.action.clone(), f.fields.clone()))
        .collect();
    let expected: Vec<(String, Vec<String>)> = expected
        .iter()
        .map(|(a, fields)| {
            (
                a.to_string(),
                fields.iter().map(|f| f.to_string()).collect(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
}

fn identity(provider: LinkedProvider, display: &str, email: Option<&str>) -> LinkedIdentity {
    LinkedIdentity {
        provider,
        external_id: "ext-1".into(),
        display_name: display.into(),
        email: email.map(Into::into),
        avatar_url: None,
        linked_at: None,
        subtitle: None,
        disconnect_key: None,
    }
}

#[tokio::test]
async fn account_page_keeps_every_form_its_action_fields_and_csrf() {
    let forest = MockForestClient::with_behavior(MockBehavior {
        list_linked_identities_result: Some(Ok(vec![identity(
            LinkedProvider::GitHub,
            "kjuulh",
            Some("kasper@example.com"),
        )])),
        ..Default::default()
    });
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_notification_preferences_result: Some(Ok(vec![NotificationPreference {
            notification_type: "NOTIFICATION_TYPE_RELEASE_FAILED".into(),
            channel: "NOTIFICATION_CHANNEL_CLI".into(),
            enabled: true,
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(forest, platform);
    let state = state
        .with_github_oauth_config(crate::state::GitHubOAuthConfig {
            client_id: "gh".into(),
            client_secret: "secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "g".into(),
            client_secret: "secret".into(),
            redirect_host: "http://localhost:3000".into(),
        });
    let cookie = create_test_session(&sessions).await;

    let page = request(&state, &cookie, "GET", "/settings/account", "").await;
    assert_eq!(page.status, StatusCode::OK);
    let html = &page.html;
    assert_clean(html);
    assert!(
        html.contains("<title>Account Settings - Forest</title>"),
        "{html}"
    );
    assert!(html.contains(r#"href="/orgs/testorg/projects" class="text-sm text-gray-500 hover:text-gray-700">&larr; testorg</a>"#), "{html}");

    let mut expected: Vec<(&str, &[&str])> = vec![
        ("/settings/account/picture", &["_csrf", "picture"]),
        ("/settings/account/username", &["_csrf", "username"]),
        ("/settings/account/emails/remove", &["_csrf", "email"]),
        ("/settings/account/emails", &["_csrf", "email"]),
        (
            "/settings/account/password",
            &[
                "_csrf",
                "current_password",
                "new_password",
                "new_password_confirm",
            ],
        ),
        ("/settings/account/mfa/setup", &["_csrf"]),
        ("/settings/account/github/disconnect", &["_csrf"]),
    ];
    for _ in 0..4 {
        expected.push((
            "/settings/account/notifications",
            &["_csrf", "notification_type", "channel", "enabled"],
        ));
    }
    assert_forms(html, &expected);

    assert!(
        html.contains(r#"enctype="multipart/form-data""#),
        "the picture upload stays multipart: {html}"
    );
    assert!(
        html.contains(r#"<form method="POST" action="/settings/account/github/disconnect" data-confirm="Disconnect this GitHub account?">"#),
        "{html}"
    );
    assert!(html.contains("kjuulh · kasper@example.com"), "{html}");
    assert!(
        !html.contains(r#"href="/settings/account/github/connect""#),
        "{html}"
    );
    assert!(
        html.contains(r#"href="/settings/account/google/connect""#),
        "{html}"
    );
    assert!(!html.contains("/settings/account/slack/connect"), "{html}");
    let failed = forms(html)
        .into_iter()
        .find(|f| {
            f.tag
                .contains(r#"value="NOTIFICATION_TYPE_RELEASE_FAILED""#)
                && f.tag.contains(r#"value="NOTIFICATION_CHANNEL_CLI""#)
        })
        .unwrap();
    assert!(
        failed.tag.contains(r#"name="enabled" value="false""#),
        "an enabled CLI preference posts enabled=false: {}",
        failed.tag
    );
    assert!(
        html.contains(r#"title="Disable release failed via CLI""#),
        "{html}"
    );
    assert!(
        html.contains(r#"title="Enable release started via CLI""#),
        "{html}"
    );
    insta::assert_snapshot!(masked_main(html));
}

#[tokio::test]
async fn account_page_with_mfa_enabled_offers_disable_with_a_code_field() {
    let forest = MockForestClient::with_behavior(MockBehavior {
        get_user_result: Some(Ok(User {
            mfa_enabled: true,
            ..ok_user()
        })),
        ..Default::default()
    });
    let (state, cookie) = session(test_state_with(forest, MockPlatformClient::new())).await;
    let page = request(&state, &cookie, "GET", "/settings/account", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    let mfa: Vec<Form> = forms(&page.html)
        .into_iter()
        .filter(|f| f.action.starts_with("/settings/account/mfa"))
        .collect();
    assert_eq!(mfa.len(), 1);
    assert_eq!(mfa[0].action, "/settings/account/mfa/disable");
    assert_eq!(mfa[0].fields, vec!["_csrf", "code"]);
    assert!(
        mfa[0].tag.contains(r#"pattern="[0-9]{6}""#),
        "{}",
        mfa[0].tag
    );
    assert!(mfa[0].tag.contains(r#"autocomplete="one-time-code""#));
    assert!(
        !page.html.contains("Linked accounts"),
        "no providers, no links: {}",
        page.html
    );
}

#[tokio::test]
async fn account_page_offers_resend_only_for_unverified_emails() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_unverified_email(&sessions).await;
    let page = request(&state, &cookie, "GET", "/settings/account", "").await;
    assert_eq!(page.status, StatusCode::OK);
    let resend: Vec<Form> = forms(&page.html)
        .into_iter()
        .filter(|f| f.action == "/settings/account/emails/resend-verification")
        .collect();
    assert_eq!(resend.len(), 1, "{}", page.html);
    assert_eq!(resend[0].fields, vec!["_csrf", "email"]);
    assert!(page.html.contains(">unverified<"));
}

#[tokio::test]
async fn account_error_rerender_shows_the_error_and_escapes_it() {
    let (state, cookie) = session(test_state()).await;
    let page = request(
        &state,
        &cookie,
        "POST",
        "/settings/account/password",
        "_csrf=test-csrf&current_password=a&new_password=b&new_password_confirm=c",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert!(page.html.contains(r#"<div class="mb-6 p-3 bg-red-50 border border-red-200 rounded-md text-sm text-red-700">New passwords do not match.</div>"#), "{}", page.html);
    assert!(
        forms(&page.html)
            .iter()
            .any(|f| f.action == "/settings/account/password")
    );
}

#[tokio::test]
async fn account_flash_and_oauth_error_come_from_known_codes_only() {
    let (state, cookie) = session(test_state()).await;
    let page = request(
        &state,
        &cookie,
        "GET",
        "/settings/account?flash=verification_resent&error=%3Cscript%3E",
        "",
    )
    .await;
    assert!(
        page.html
            .contains("Verification email sent. Check your inbox.")
    );
    assert!(!page.html.contains("bg-amber-50"), "{}", page.html);
    assert!(!page.html.contains("<script>"));
}

#[tokio::test]
async fn created_token_is_shown_once_and_never_cached() {
    let forest = MockForestClient::with_behavior(MockBehavior {
        list_tokens_result: Some(Ok(vec![PersonalAccessToken {
            token_id: "tok-1".into(),
            name: "CI <deploy>".into(),
            scopes: vec![],
            created_at: Some("2026-01-01T00:00:00Z".into()),
            last_used: None,
            expires_at: Some(String::new()),
        }])),
        ..Default::default()
    });
    let (state, cookie) = session(test_state_with(forest, MockPlatformClient::new())).await;

    let created = request(
        &state,
        &cookie,
        "POST",
        "/settings/tokens",
        "_csrf=test-csrf&name=ci",
    )
    .await;
    assert_eq!(created.status, StatusCode::OK);
    assert_clean(&created.html);
    assert!(
        created.html.contains("forg_abcdef1234567890"),
        "{}",
        created.html
    );
    assert!(created.html.contains("it won't be shown again"));
    assert_eq!(
        created
            .headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    insta::assert_snapshot!("created_token", masked_main(&created.html));

    let listed = request(&state, &cookie, "GET", "/settings/tokens", "").await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_clean(&listed.html);
    assert!(
        !listed.html.contains("forg_abcdef1234567890"),
        "{}",
        listed.html
    );
    assert!(!listed.html.contains("Token created successfully"));
    assert!(listed.html.contains("CI &lt;deploy&gt;"), "{}", listed.html);
    assert!(
        listed
            .html
            .contains("<td class=\"px-6 py-3 text-gray-600\">Never</td>"),
        "{}",
        listed.html
    );
    assert_forms(
        &listed.html,
        &[
            ("/settings/tokens", &["_csrf", "name"]),
            ("/settings/tokens/tok-1/delete", &["_csrf"]),
        ],
    );
    insta::assert_snapshot!("token_list", masked_main(&listed.html));
}

#[tokio::test]
async fn tokens_page_without_tokens_shows_the_empty_state() {
    let (state, cookie) = session(test_state()).await;
    let page = request(&state, &cookie, "GET", "/settings/tokens", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("No tokens yet."));
    assert!(
        page.html
            .contains("Create one above to use with the forest CLI.")
    );
    assert_forms(&page.html, &[("/settings/tokens", &["_csrf", "name"])]);
}

#[tokio::test]
async fn authorized_apps_revoke_confirms_through_data_confirm() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_app(
        "cid",
        vec!["https://app.test/cb".into()],
        vec!["profile".into(), "email".into()],
    );
    let (state, cookie) = session(test_state_with_oauth_apps(oauth)).await;
    let page = request(&state, &cookie, "GET", "/settings/authorized-apps", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert!(page.html.contains(r#"data-confirm="Revoke access for Seeded App? It will no longer be able to act on your behalf.""#), "{}", page.html);
    assert_forms(
        &page.html,
        &[("/settings/authorized-apps/app-1/revoke", &["_csrf"])],
    );
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn authorized_apps_without_grants_says_so() {
    let (state, cookie) = session(test_state_with_oauth_apps(Arc::new(
        MockOAuthAppsClient::new(),
    )))
    .await;
    let page = request(&state, &cookie, "GET", "/settings/authorized-apps", "").await;
    assert!(
        page.html
            .contains("You haven&#39;t authorized any applications.")
            || page
                .html
                .contains("You haven't authorized any applications."),
        "{}",
        page.html
    );
}

#[tokio::test]
async fn mfa_setup_renders_our_qr_svg_without_inline_style() {
    let (state, cookie) = session(test_state()).await;
    let page = request(
        &state,
        &cookie,
        "POST",
        "/settings/account/mfa/setup",
        "_csrf=test-csrf",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert_eq!(
        page.headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert!(
        page.html.contains("<svg xmlns="),
        "the QR svg is markup, not text: {}",
        page.html
    );
    assert!(!page.html.contains("&lt;svg"), "{}", page.html);
    assert!(
        page.html.contains("[&amp;_svg]:w-[200px]") || page.html.contains("[&_svg]:w-[200px]"),
        "{}",
        page.html
    );
    assert!(page.html.contains("JBSWY3DPEHPK3PXP"));
    assert_forms(
        &page.html,
        &[("/settings/account/mfa/verify", &["_csrf", "mfa_id", "code"])],
    );
    assert!(
        page.html
            .contains(r#"<input type="hidden" name="mfa_id" value="mfa-mock-1">"#)
    );
    assert!(page.html.contains(r#"pattern="[0-9]{6}""#));
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn mfa_setup_requires_csrf() {
    let (state, cookie) = session(test_state()).await;
    let page = request(
        &state,
        &cookie,
        "POST",
        "/settings/account/mfa/setup",
        "_csrf=wrong",
    )
    .await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
}

fn my_artifact() -> Artifact {
    Artifact {
        artifact_id: "art-9".into(),
        slug: "api-\"quoted\"".into(),
        context: ArtifactContext {
            title: "Ship it".into(),
            description: Some("Adds <b>things</b>".into()),
            web: None,
            pr: None,
        },
        source: Some(ArtifactSource {
            user: Some("testuser".into()),
            email: None,
            source_type: None,
            run_url: None,
        }),
        git_ref: Some(ArtifactRef {
            commit_sha: "0123456789abcdef".into(),
            branch: Some("main".into()),
            commit_message: Some("feat: ship".into()),
            version: Some("v1.2.0".into()),
            repo_url: None,
        }),
        destinations: vec![],
        created_at: "2026-03-07T12:00:00Z".into(),
    }
}

#[tokio::test]
async fn notifications_page_loads_its_script_and_lists_my_releases() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![my_artifact()])),
        ..Default::default()
    });
    let (state, cookie) = session(test_state_with(MockForestClient::new(), platform)).await;
    let page = request(&state, &cookie, "GET", "/notifications", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert!(
        regex::Regex::new(
            r#"<script src="/static/js/notifications\.js\?v=[0-9a-z]+" defer></script>"#
        )
        .unwrap()
        .is_match(&page.html),
        "{}",
        page.html
    );
    assert!(page.html.contains(r#"<div id="notifications-list">"#));
    assert!(
        page.html.contains(r#"data-slug="api-&quot;quoted&quot;""#),
        "{}",
        page.html
    );
    assert!(
        page.html.contains("Adds &lt;b&gt;things&lt;/b&gt;"),
        "{}",
        page.html
    );
    assert!(page.html.contains(r#"href="/users/testuser""#));
    assert!(page.html.contains(">feat: ship</a>"), "{}", page.html);
    assert!(page.html.contains(">0123456<"), "{}", page.html);
    insta::assert_snapshot!(masked_main(&page.html));
}

#[tokio::test]
async fn notifications_partial_is_only_the_list() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![my_artifact()])),
        ..Default::default()
    });
    let (state, cookie) = session(test_state_with(MockForestClient::new(), platform)).await;
    let page = request(&state, &cookie, "GET", "/notifications?_partial=1", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.html.contains("<html"), "{}", page.html);
    assert!(!page.html.contains("<script"), "{}", page.html);
    assert!(
        page.html
            .trim_start()
            .starts_with(r#"<div class="space-y-3">"#),
        "{}",
        page.html
    );
    assert!(page.html.contains("feat: ship"));
}

#[tokio::test]
async fn notifications_without_releases_show_the_empty_state() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, cookie) = session(test_state_with(MockForestClient::new(), platform)).await;
    let page = request(&state, &cookie, "GET", "/orgs/testorg/notifications", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("No release activity yet."));
    let partial = request(&state, &cookie, "GET", "/notifications?_partial=1", "").await;
    assert!(partial.html.contains("No release activity yet."));
}

#[tokio::test]
async fn user_profile_has_no_inline_styles_and_links_releases() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![my_artifact()])),
        ..Default::default()
    });
    let (state, cookie) = session(test_state_with(MockForestClient::new(), platform)).await;
    let page = request(&state, &cookie, "GET", "/users/testuser", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert!(page.html.contains("<title>testuser - Forest</title>"));
    assert!(page.html.contains(r#"href="/orgs/testorg/projects/"#));
    assert!(page.html.contains("v1.2.0"));
    assert!(page.html.contains("Member since"));
    let main = main_element(&page.html);
    let without_heatmap = regex::Regex::new(
        r#"(?s)<div class="overflow-x-auto pb-2">.*?<span>More</span>\s*</div>\s*</div>\s*</div>"#,
    )
    .unwrap()
    .replace(&main, "HEATMAP");
    let without_heatmap = regex::Regex::new(r">\s*[0-9]+ contributions? in the last year\s*<")
        .unwrap()
        .replace(&without_heatmap, ">COUNT<");
    let masked = regex::Regex::new(
        r">\s*[0-9]+ (seconds?|minutes?|hours?|days?|months?|years?) ago\s*<|>\s*just now\s*<",
    )
    .unwrap()
    .replace_all(&without_heatmap, ">AGO<")
    .into_owned();
    insta::assert_snapshot!(masked);
}

#[tokio::test]
async fn user_profile_heatmap_cells_use_classes_and_titles() {
    let (state, cookie) = session(test_state()).await;
    let page = request(&state, &cookie, "GET", "/users/someone", "").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_clean(&page.html);
    assert!(
        page.html
            .contains(r#"class="rounded-sm bg-gray-100 size-[11px]" title="#),
        "{}",
        page.html
    );
    assert!(
        regex::Regex::new(r#"title="\d{4}-\d{2}-\d{2}: 0 contributions""#)
            .unwrap()
            .is_match(&page.html)
    );
    assert!(
        page.html
            .contains(r#"<span class="h-[11px] leading-[11px] text-[9px]">Mon</span>"#)
    );
}

#[test]
fn contribution_levels_bucket_like_the_legend() {
    assert_eq!(contribution_level(0, 0), 0);
    assert_eq!(contribution_level(0, 9), 0);
    assert_eq!(contribution_level(9, 9), 4);
    assert_eq!(contribution_level(1, 100), 1);
    assert_eq!(contribution_level(1, 9), 2);
    assert_eq!(contribution_level(4, 9), 3);
    assert_eq!(contribution_level(1, 1), 4);
}
