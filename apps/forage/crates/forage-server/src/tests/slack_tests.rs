use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::integrations::{
    CreateIntegrationInput, IntegrationConfig, IntegrationStore, IntegrationType,
};
use tower::ServiceExt;

use crate::test_support::*;

fn build_app_with_integrations() -> (
    axum::Router,
    std::sync::Arc<forage_core::session::InMemorySessionStore>,
    std::sync::Arc<forage_core::integrations::InMemoryIntegrationStore>,
) {
    let (state, sessions, integrations) =
        test_state_with_integrations(MockForestClient::new(), MockPlatformClient::new());
    let app = crate::build_router(state);
    (app, sessions, integrations)
}

// ─── Install Slack page ─────────────────────────────────────────────

#[tokio::test]
async fn install_slack_page_returns_200() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations/install/slack")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Install Slack"));
    assert!(text.contains("Webhook URL"));
}

#[tokio::test]
async fn install_slack_page_returns_403_for_non_admin() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session_member(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations/install/slack")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn install_slack_page_shows_manual_form_without_oauth() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations/install/slack")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    // Should show manual webhook URL form
    assert!(text.contains("hooks.slack.com"));
    // Should NOT show "Add to Slack" button (no OAuth configured)
    assert!(!text.contains("Add to Slack"));
}

// ─── Create Slack (manual webhook URL) ──────────────────────────────

#[tokio::test]
async fn create_slack_success_shows_installed_page() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=test-csrf&name=%23deploys&webhook_url=https%3A%2F%2Fhooks.slack.com%2Fservices%2FT123%2FB456%2Fxyz&channel_name=%23deploys";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/slack")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("installed"));
    assert!(text.contains("fgi_")); // API token shown
    assert!(text.contains("#deploys"));

    // Verify it was created as Slack type
    let all = integrations.list_integrations("testorg").await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "#deploys");
    match &all[0].config {
        IntegrationConfig::Slack { channel_name, webhook_url, .. } => {
            assert_eq!(channel_name, "#deploys");
            assert!(webhook_url.contains("hooks.slack.com"));
        }
        _ => panic!("expected Slack config"),
    }
}

#[tokio::test]
async fn create_slack_defaults_channel_to_general() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=test-csrf&name=alerts&webhook_url=https%3A%2F%2Fhooks.slack.com%2Fservices%2FT123%2FB456%2Fxyz&channel_name=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/slack")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let all = integrations.list_integrations("testorg").await.unwrap();
    match &all[0].config {
        IntegrationConfig::Slack { channel_name, .. } => {
            assert_eq!(channel_name, "#general");
        }
        _ => panic!("expected Slack config"),
    }
}

#[tokio::test]
async fn create_slack_invalid_csrf_returns_403() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=wrong-csrf&name=%23deploys&webhook_url=https%3A%2F%2Fhooks.slack.com%2Fservices%2FT123%2FB456%2Fxyz&channel_name=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/slack")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_slack_rejects_non_slack_url() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=test-csrf&name=%23deploys&webhook_url=https%3A%2F%2Fexample.com%2Fhook&channel_name=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/slack")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should redirect back to install page with error
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("install/slack"));
    assert!(location.contains("error="));
}

#[tokio::test]
async fn create_slack_non_admin_returns_403() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session_member(&sessions).await;

    let body = "_csrf=test-csrf&name=%23deploys&webhook_url=https%3A%2F%2Fhooks.slack.com%2Fservices%2FT123%2FB456%2Fxyz&channel_name=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/slack")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_slack_rejects_empty_name() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=test-csrf&name=&webhook_url=https%3A%2F%2Fhooks.slack.com%2Fservices%2FT123%2FB456%2Fxyz&channel_name=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/slack")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should redirect back with error
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("install/slack"));
    assert!(location.contains("error="));
}

// ─── Slack integration detail ───────────────────────────────────────

#[tokio::test]
async fn slack_integration_detail_shows_config() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Slack,
            name: "#deploys".into(),
            config: IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "My Team".into(),
                channel_id: "C456".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}",
                    created.id
                ))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("#deploys"));
    assert!(text.contains("My Team"));
    assert!(text.contains("Slack"));
}

#[tokio::test]
async fn slack_integration_detail_manual_mode_shows_webhook_url() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    // Manual mode: empty team_name
    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Slack,
            name: "manual-slack".into(),
            config: IntegrationConfig::Slack {
                team_id: String::new(),
                team_name: String::new(),
                channel_id: String::new(),
                channel_name: "#deploys".into(),
                access_token: String::new(),
                webhook_url: "https://hooks.slack.com/services/T123/B456/xyz".into(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}",
                    created.id
                ))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("hooks.slack.com"));
}

// ─── Slack in integrations catalog ──────────────────────────────────

#[tokio::test]
async fn integrations_page_shows_slack_as_available() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    // Slack should be a clickable link to install page
    assert!(text.contains("install/slack"));
}

// ─── Slack shows in installed list ──────────────────────────────────

#[tokio::test]
async fn integrations_page_shows_installed_slack() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Slack,
            name: "#alerts".into(),
            config: IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "Test".into(),
                channel_id: "C456".into(),
                channel_name: "#alerts".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("#alerts"));
    assert!(text.contains("Slack"));
}

// ─── Slack OAuth callback without session ───────────────────────────

#[tokio::test]
async fn slack_callback_without_state_returns_error() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/integrations/slack/callback?code=test-code")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn slack_callback_with_error_redirects() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/integrations/slack/callback?state=testorg&error=access_denied")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("install/slack"));
    assert!(location.contains("error="));
    assert!(location.contains("access_denied"));
}

#[tokio::test]
async fn slack_callback_without_oauth_config_returns_503() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/integrations/slack/callback?code=test-code&state=testorg")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // No SlackConfig set, so should return 503
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ─── Reinstall Slack ─────────────────────────────────────────────────

#[tokio::test]
async fn reinstall_slack_redirects_to_oauth_error_without_slack_config() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Slack,
            name: "#deploys".into(),
            config: IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "My Team".into(),
                channel_id: "C456".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let body = format!("_csrf=test-csrf");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/reinstall",
                    created.id
                ))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // No SlackConfig set → 503
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn reinstall_slack_non_admin_returns_403() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session_member(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Slack,
            name: "#deploys".into(),
            config: IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "My Team".into(),
                channel_id: "C456".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let body = format!("_csrf=test-csrf");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/reinstall",
                    created.id
                ))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reinstall_slack_invalid_csrf_returns_403() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Slack,
            name: "#deploys".into(),
            config: IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "My Team".into(),
                channel_id: "C456".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let body = format!("_csrf=wrong-csrf");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/reinstall",
                    created.id
                ))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
