//! Route tests for organisation-wide rule sets.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{OrgPolicyRule, OrgRuleSet, PolicyConfig, ProjectSelector};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

async fn body_to_string(body: Body) -> String {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn org_rules_page_renders_rule_sets_and_nav() {
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("domain".into(), "retail".into());
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_org_rule_sets_result: Some(Ok(vec![OrgRuleSet {
            organisation: "testorg".into(),
            name: "retail-prod".into(),
            enabled: true,
            selector: ProjectSelector {
                include_projects: vec!["butikkaerlighilsen".into()],
                exclude_projects: vec!["sandbox".into()],
                name_regex: Some("^butikk".into()),
                metadata_match: metadata,
                tags: vec!["web".into(), "postgres".into()],
            },
            policies: vec![OrgPolicyRule {
                name: "prod-main-only".into(),
                enabled: true,
                config: PolicyConfig::BranchRestriction {
                    target_environment: "prod".into(),
                    branch_pattern: "^main$".into(),
                },
            }],
            triggers: vec![],
            release_pipelines: vec![],
            created_at: "2026-08-30T00:00:00Z".into(),
            updated_at: "2026-08-30T00:00:00Z".into(),
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/rules")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response.into_body()).await;
    assert!(html.contains("Organisation Rules"));
    assert!(html.contains("retail-prod"));
    assert!(html.contains("prod-main-only"));
    assert!(html.contains("/orgs/testorg/rules"));
}

#[tokio::test]
async fn org_rules_create_rejects_invalid_json() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "csrf_token=test-csrf&name=bad&policies_json=not-json";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/rules")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = body_to_string(response.into_body()).await;
    assert!(html.contains("Invalid policies JSON"));
}
