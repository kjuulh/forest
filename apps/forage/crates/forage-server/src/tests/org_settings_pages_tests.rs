use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::integrations::slack_release::EMOJI_SLOTS;
use forage_core::integrations::{
    CreateIntegrationInput, DeliveryStatus, InMemoryIntegrationStore, IntegrationConfig,
    IntegrationStore, IntegrationType,
};
use tower::ServiceExt;

use crate::state::SlackConfig;
use crate::test_support::*;

struct App {
    router: axum::Router,
    cookie: String,
    store: Arc<InMemoryIntegrationStore>,
}

async fn app(slack_oauth: bool) -> App {
    let (state, sessions, store) =
        test_state_with_integrations(MockForestClient::new(), MockPlatformClient::new());
    let state = if slack_oauth {
        state.with_slack_config(SlackConfig {
            client_id: "cid".into(),
            client_secret: "secret".into(),
            redirect_host: "https://forest.test".into(),
        })
    } else {
        state
    };
    let cookie = create_test_session(&sessions).await;
    App {
        router: crate::build_router(state),
        cookie,
        store,
    }
}

impl App {
    async fn get(&self, uri: &str) -> (StatusCode, String) {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("cookie", &self.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    async fn post(&self, uri: &str, form: &str) -> (StatusCode, String) {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", &self.cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    async fn create(&self, kind: IntegrationType, name: &str, config: IntegrationConfig) -> String {
        self.store
            .create_integration(&CreateIntegrationInput {
                organisation: "testorg".into(),
                integration_type: kind,
                name: name.into(),
                config,
                created_by: "user-123".into(),
            })
            .await
            .unwrap()
            .id
    }

    async fn webhook(&self, name: &str, secret: Option<&str>) -> String {
        self.create(
            IntegrationType::Webhook,
            name,
            IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: secret.map(Into::into),
                headers: HashMap::new(),
            },
        )
        .await
    }

    async fn slack(&self) -> String {
        self.create(
            IntegrationType::Slack,
            "#deploys",
            IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "My Team".into(),
                channel_id: "C456".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
        )
        .await
    }
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
    let main = regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .unwrap()
        .replace_all(&main, "ID");
    let main = regex::Regex::new(r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}")
        .unwrap()
        .replace_all(&main, "TIME");
    regex::Regex::new(r"fgi_[A-Za-z0-9_-]+")
        .unwrap()
        .replace_all(&main, "TOKEN")
        .into_owned()
}

fn assert_clean(html: &str) {
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    assert!(!html.contains("onsubmit"), "no inline handlers: {html}");
    assert!(!html.contains("onclick"), "no inline handlers: {html}");
    assert!(!html.contains("style="), "no inline styles: {html}");
    assert!(
        !main_element(html).contains(">none<"),
        "no none leak: {html}"
    );
    assert!(
        !main_element(html).contains("value=\"none\""),
        "no none leak: {html}"
    );
    assert!(
        !main_element(html).contains("/none"),
        "no none leak: {html}"
    );
}

#[tokio::test]
async fn org_settings_shows_name_role_and_the_settings_sidebar() {
    let app = app(false).await;
    let (status, html) = app.get("/orgs/testorg/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(
        html.contains("<title>testorg - Settings - Forest</title>"),
        "{html}"
    );
    assert!(
        html.contains(r#"<dd class="font-medium">testorg</dd>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<dd class="font-medium">owner</dd>"#),
        "{html}"
    );
    for href in [
        "/orgs/testorg/settings",
        "/orgs/testorg/settings/members",
        "/orgs/testorg/settings/integrations",
        "/orgs/testorg/settings/access",
        "/orgs/testorg/settings/visibility",
        "/orgs/testorg/settings/rules",
        "/orgs/testorg/settings/developers",
    ] {
        assert!(
            html.contains(&format!(r#"href="{href}""#)),
            "{href}: {html}"
        );
    }
    assert!(
        html.contains(r#"<a href="/orgs/testorg/settings" aria-current="page" class="block px-3 py-1.5 text-sm rounded-md bg-gray-100"#),
        "General is the current section: {html}"
    );
    assert!(
        html.contains(
            r#"<a href="/orgs/testorg/settings" aria-current="page" class="whitespace-nowrap"#
        ),
        "the Settings tab is current: {html}"
    );
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn org_settings_refuses_a_non_member() {
    let app = app(false).await;
    let (status, _) = app.get("/orgs/otherorg/settings").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn integrations_lists_installed_integrations_and_what_can_be_added() {
    let app = app(false).await;
    let webhook = app.webhook("Production alerts", None).await;
    app.store
        .set_rule_enabled(&webhook, "release_failed", false)
        .await
        .unwrap();
    let paused = app.slack().await;
    app.store
        .set_integration_enabled("testorg", &paused, false)
        .await
        .unwrap();
    let (status, html) = app
        .get("/orgs/testorg/settings/integrations?error=Slack+said+no")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(
        html.contains(&format!(
            r#"href="/orgs/testorg/settings/integrations/{webhook}""#
        )),
        "{html}"
    );
    assert!(html.contains("Webhook · 3/4 events"), "{html}");
    assert!(html.contains("Slack · All events"), "{html}");
    assert!(html.contains(">Paused</span>"), "{html}");
    assert!(html.contains(">Active</span>"), "{html}");
    assert!(
        html.contains("Slack said no"),
        "the error query is shown: {html}"
    );
    for href in ["install/webhook", "install/slack", "install/github"] {
        assert!(
            html.contains(&format!(
                r#"href="/orgs/testorg/settings/integrations/{href}""#
            )),
            "{href}: {html}"
        );
    }
    assert!(
        html.contains(r#"<a href="/orgs/testorg/settings/integrations" aria-current="page""#),
        "Integrations is the current section: {html}"
    );
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn integrations_with_none_installed_shows_only_the_catalogue() {
    let app = app(false).await;
    let (_, html) = app.get("/orgs/testorg/settings/integrations").await;
    assert!(!html.contains("Installed</h2>"), "{html}");
    assert!(
        !html.contains("bg-red-50"),
        "no error box without an error: {html}"
    );
    assert!(html.contains("Coming soon"), "{html}");
}

#[tokio::test]
async fn slack_integration_detail_offers_every_form_with_the_csrf_field() {
    let app = app(true).await;
    let id = app.slack().await;
    let base = format!("/orgs/testorg/settings/integrations/{id}");
    let (status, html) = app.get(&base).await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    for action in [
        "reinstall",
        "toggle",
        "delete",
        "rules",
        "baseline",
        "emoji",
        "test",
    ] {
        assert!(
            html.contains(&format!(r#"<form method="POST" action="{base}/{action}""#)),
            "{action}: {html}"
        );
    }
    let forms = html
        .matches("<form method=\"POST\" action=\"/orgs/testorg/settings/integrations/")
        .count();
    let csrf = html
        .matches(r#"<input type="hidden" name="_csrf" value="test-csrf">"#)
        .count();
    assert_eq!(
        forms, 10,
        "reinstall, toggle, delete, 4 rules, baseline, emoji, test"
    );
    assert_eq!(
        csrf,
        forms + 2,
        "every form carries _csrf, plus the two sign-out forms"
    );
    assert!(
        html.contains(r#"action="/orgs/testorg/settings/integrations/"#) && html.contains(r#"/delete" class="inline" data-confirm="Uninstall this integration? This cannot be undone.""#),
        "{html}"
    );
    assert!(
        html.contains(r#"<input type="hidden" name="enabled" value="false">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<input type="hidden" name="notification_type" value="release_failed">"#),
        "{html}"
    );
    assert!(html.contains(r#"title="Disable release failed""#), "{html}");
    assert!(html.contains(r#"name="directory_mapping""#), "{html}");
    for nt in ["release_started", "release_succeeded", "release_failed"] {
        assert!(
            html.contains(&format!(
                r#"<input type="checkbox" name="dm_{nt}" checked>"#
            )),
            "{nt}: {html}"
        );
    }
    assert!(
        html.contains("Not configured yet: personal Slack links only."),
        "{html}"
    );
    for (field, fallback, _) in EMOJI_SLOTS {
        assert!(
            html.contains(&format!(r#"<input type="text" name="{field}" value="" placeholder="{fallback}" maxlength="100""#)),
            "{field}: {html}"
        );
    }
    assert!(html.contains("My Team · #deploys"), "{html}");
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn saving_status_emoji_shows_them_and_a_confirmation() {
    let app = app(false).await;
    let id = app.slack().await;
    let base = format!("/orgs/testorg/settings/integrations/{id}");
    let (status, _) = app
        .post(
            &format!("{base}/emoji"),
            "_csrf=test-csrf&running=party-parrot",
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = app.get(&format!("{base}?saved=emoji")).await;
    assert!(html.contains("Status emoji saved."), "{html}");
    assert!(
        !html.contains("Individual notification settings saved."),
        "{html}"
    );
    assert!(
        html.contains(r#"name="running" value="party-parrot""#),
        "{html}"
    );
    assert!(
        !html.contains(r#"/reinstall""#),
        "no reinstall without Slack OAuth: {html}"
    );
}

#[tokio::test]
async fn detail_flashes_follow_the_query() {
    let app = app(false).await;
    let id = app.slack().await;
    let base = format!("/orgs/testorg/settings/integrations/{id}");
    let (_, html) = app
        .get(&format!(
            "{base}?saved=baseline&test=sent&error=Emoji+names+%3Cb%3E"
        ))
        .await;
    assert!(
        html.contains("Individual notification settings saved."),
        "{html}"
    );
    assert!(html.contains("Test notification sent."), "{html}");
    assert!(
        html.contains("Emoji names &lt;b&gt;"),
        "the error is escaped: {html}"
    );
    assert!(!html.contains("Status emoji saved."), "{html}");
    let (_, html) = app.get(&format!("{base}?error=")).await;
    assert!(
        !html.contains("text-red-700 bg-red-50")
            && !html.contains("bg-red-50 border-red-200 text-red-700"),
        "an empty error shows nothing: {html}"
    );
}

#[tokio::test]
async fn webhook_detail_shows_signing_and_deliveries_without_slack_sections() {
    let app = app(false).await;
    let id = app.webhook("delivery-test", Some("whsec_x")).await;
    app.store
        .record_delivery(
            &id,
            "notif-aaaaaaaaaaaaaaaa",
            DeliveryStatus::Failed,
            Some("HTTP 500: Internal Server Error"),
        )
        .await
        .unwrap();
    app.store
        .record_delivery(&id, "notif-b", DeliveryStatus::Skipped, None)
        .await
        .unwrap();
    let (status, html) = app
        .get(&format!("/orgs/testorg/settings/integrations/{id}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("HMAC-SHA256 enabled"), "{html}");
    assert!(
        html.contains(r#"title="notif-aaaaaaaaaaaaaaaa">notif-aaaaaa…</td>"#),
        "{html}"
    );
    assert!(html.contains(r#"title="notif-b">notif-b</td>"#), "{html}");
    assert!(html.contains("HTTP 500: Internal Server Error"), "{html}");
    assert!(html.contains(">Skipped</span>"), "{html}");
    assert!(!html.contains(r#"id="emoji""#), "{html}");
    assert!(!html.contains(r#"id="individual""#), "{html}");
    assert!(!html.contains(r#"/reinstall""#), "{html}");
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn install_webhook_form_posts_name_url_and_secret() {
    let app = app(false).await;
    let (status, html) = app
        .get("/orgs/testorg/settings/integrations/install/webhook")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains(r#"<form method="POST" action="/orgs/testorg/settings/integrations/webhook" class="space-y-5">"#), "{html}");
    assert!(html.contains(r#"<input type="text" id="name" name="name" placeholder="e.g. Production alerts" required"#), "{html}");
    assert!(html.contains(r#"<input type="url" id="url" name="url" placeholder="https://example.com/webhooks/forage" required"#), "{html}");
    assert!(
        html.contains(
            r#"<input type="text" id="secret" name="secret" placeholder="whsec_..." class"#
        ),
        "{html}"
    );
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn install_slack_offers_oauth_only_when_configured() {
    let app = app(true).await;
    let (status, html) = app
        .get("/orgs/testorg/settings/integrations/install/slack?error=Denied")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains(r#"<form method="POST" action="/orgs/testorg/settings/integrations/install/slack/start">"#), "{html}");
    assert!(html.contains(r#"<form method="POST" action="/orgs/testorg/settings/integrations/slack" class="space-y-5">"#), "{html}");
    for name in ["name", "webhook_url", "channel_name"] {
        assert!(
            html.contains(&format!(r#"name="{name}""#)),
            "{name}: {html}"
        );
    }
    assert!(html.contains("Denied"), "{html}");
    insta::assert_snapshot!(masked_main(&html));

    let app = self::app(false).await;
    let (_, html) = app
        .get("/orgs/testorg/settings/integrations/install/slack")
        .await;
    assert!(!html.contains("/install/slack/start"), "{html}");
    assert!(
        html.contains("under <strong>Incoming Webhooks</strong>"),
        "{html}"
    );
}

#[tokio::test]
async fn install_github_says_when_the_app_is_not_configured() {
    let app = app(false).await;
    let (status, html) = app
        .get("/orgs/testorg/settings/integrations/install/github")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(
        html.contains("GitHub organisation connections are not configured on this server yet."),
        "{html}"
    );
    assert!(
        !html.contains(r#"action="/orgs/testorg/settings/integrations/github""#),
        "{html}"
    );
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn installed_page_shows_the_token_once_with_a_copy_button() {
    let app = app(false).await;
    let (status, html) = app
        .post(
            "/orgs/testorg/settings/integrations/webhook",
            "_csrf=test-csrf&name=my-hook&url=https%3A%2F%2Fexample.com%2Fhook&secret=",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(
        html.contains("<title>my-hook installed - Forest</title>"),
        "{html}"
    );
    assert!(html.contains(r#"<code id="api-token""#), "{html}");
    assert!(html.contains(r#"data-copy="api-token""#), "{html}");
    assert!(
        html.contains(r#"href="/orgs/testorg/settings/integrations""#),
        "{html}"
    );
    insta::assert_snapshot!(masked_main(&html));
}
