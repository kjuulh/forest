use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::integrations::{
    CreateIntegrationInput, DeliveryStatus, IntegrationConfig, IntegrationStore, IntegrationType,
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

// ─── List integrations ──────────────────────────────────────────────

#[tokio::test]
async fn integrations_page_returns_200_for_admin() {
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
    assert!(text.contains("Integrations"));
    assert!(text.contains("Available integrations"));
}

#[tokio::test]
async fn integrations_page_returns_403_for_non_admin() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session_member(&sessions).await;

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

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn integrations_page_returns_403_for_non_member() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/otherorg/settings/integrations")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn integrations_page_shows_existing_integrations() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    // Create a webhook integration
    integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "Production alerts".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
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
    assert!(text.contains("Production alerts"));
    assert!(text.contains("Webhook"));
}

// ─── Install webhook page ───────────────────────────────────────────

#[tokio::test]
async fn install_webhook_page_returns_200() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations/install/webhook")
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
    assert!(text.contains("Install Webhook"));
    assert!(text.contains("Payload URL"));
}

#[tokio::test]
async fn install_webhook_page_returns_403_for_non_admin() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session_member(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations/install/webhook")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ─── Create webhook ─────────────────────────────────────────────────

#[tokio::test]
async fn create_webhook_success_shows_installed_page() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=test-csrf&name=my-hook&url=https%3A%2F%2Fexample.com%2Fhook&secret=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/webhook")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    // Renders the "installed" page directly (with API token shown once)
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("installed"));
    assert!(text.contains("fgi_")); // API token shown
    assert!(text.contains("my-hook"));

    // Verify it was created
    let all = integrations.list_integrations("testorg").await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "my-hook");
}

#[tokio::test]
async fn create_webhook_invalid_csrf_returns_403() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=wrong-csrf&name=my-hook&url=https%3A%2F%2Fexample.com%2Fhook&secret=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/webhook")
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
async fn create_webhook_rejects_http_url() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let body = "_csrf=test-csrf&name=my-hook&url=http%3A%2F%2Fexample.com%2Fhook&secret=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/webhook")
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
    assert!(location.contains("install/webhook"));
    assert!(location.contains("error="));
}

#[tokio::test]
async fn create_webhook_non_admin_returns_403() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session_member(&sessions).await;

    let body = "_csrf=test-csrf&name=my-hook&url=https%3A%2F%2Fexample.com%2Fhook&secret=";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/integrations/webhook")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ─── Integration detail ─────────────────────────────────────────────

#[tokio::test]
async fn integration_detail_returns_200() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "test-hook".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: Some("s3cret".into()),
                headers: std::collections::HashMap::new(),
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
    assert!(text.contains("test-hook"));
    assert!(text.contains("Release failed"));
    assert!(text.contains("HMAC-SHA256 enabled"));
}

#[tokio::test]
async fn integration_detail_not_found_returns_404() {
    let (app, sessions, _) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/integrations/00000000-0000-0000-0000-000000000000")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─── Toggle integration ─────────────────────────────────────────────

#[tokio::test]
async fn toggle_integration_disables_and_enables() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "toggle-test".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    // Disable
    let body = format!("_csrf=test-csrf&enabled=false");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/toggle",
                    created.id
                ))
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let integ = integrations
        .get_integration("testorg", &created.id)
        .await
        .unwrap();
    assert!(!integ.enabled);
}

// ─── Delete integration ─────────────────────────────────────────────

#[tokio::test]
async fn delete_integration_removes_it() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "delete-test".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let body = "_csrf=test-csrf";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/delete",
                    created.id
                ))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let all = integrations.list_integrations("testorg").await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn delete_integration_invalid_csrf_returns_403() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "csrf-test".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    let body = "_csrf=wrong-csrf";
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/delete",
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
    // Verify it was NOT deleted
    let all = integrations.list_integrations("testorg").await.unwrap();
    assert_eq!(all.len(), 1);
}

// ─── Update notification rules ──────────────────────────────────────

#[tokio::test]
async fn update_rule_toggles_notification_type() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "rule-test".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    // Disable release_failed
    let body = format!(
        "_csrf=test-csrf&notification_type=release_failed&enabled=false"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!(
                    "/orgs/testorg/settings/integrations/{}/rules",
                    created.id
                ))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let rules = integrations.list_rules(&created.id).await.unwrap();
    let failed_rule = rules
        .iter()
        .find(|r| r.notification_type == "release_failed")
        .unwrap();
    assert!(!failed_rule.enabled);

    // Other rules should still be enabled
    let started_rule = rules
        .iter()
        .find(|r| r.notification_type == "release_started")
        .unwrap();
    assert!(started_rule.enabled);
}

// ─── Delivery log ──────────────────────────────────────────────────

#[tokio::test]
async fn detail_page_shows_delivery_log() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "delivery-test".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
            },
            created_by: "user-123".into(),
        })
        .await
        .unwrap();

    // Record a successful and a failed delivery
    integrations
        .record_delivery(&created.id, "notif-aaa", DeliveryStatus::Delivered, None)
        .await
        .unwrap();
    integrations
        .record_delivery(
            &created.id,
            "notif-bbb",
            DeliveryStatus::Failed,
            Some("HTTP 500: Internal Server Error"),
        )
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

    // Should show the deliveries section
    assert!(text.contains("Recent deliveries"));
    assert!(text.contains("Delivered"));
    assert!(text.contains("Failed"));
    assert!(text.contains("notif-aaa"));
    assert!(text.contains("notif-bbb"));
    assert!(text.contains("HTTP 500: Internal Server Error"));
}

#[tokio::test]
async fn detail_page_shows_empty_deliveries() {
    let (app, sessions, integrations) = build_app_with_integrations();
    let cookie = create_test_session(&sessions).await;

    let created = integrations
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "empty-delivery-test".into(),
            config: IntegrationConfig::Webhook {
                url: "https://example.com/hook".into(),
                secret: None,
                headers: std::collections::HashMap::new(),
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
    assert!(text.contains("No deliveries yet"));
}
