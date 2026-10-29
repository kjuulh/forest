use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{PlatformError, Visibility};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

async fn body_to_string(body: Body) -> String {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn platform(visibility: Visibility, calls: &Arc<Mutex<Vec<String>>>) -> MockPlatformClient {
    MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_visibility_result: Some(Ok(visibility)),
        visibility_calls: Some(calls.clone()),
        ..Default::default()
    })
}

fn vis(organisation: &str, project: Option<&str>, can_change: bool) -> Visibility {
    Visibility {
        organisation: organisation.into(),
        project: project.map(Into::into),
        can_change,
    }
}

async fn get(app: axum::Router, uri: &str, cookie: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_to_string(response.into_body()).await)
}

async fn post(
    app: axum::Router,
    uri: &str,
    cookie: &str,
    form: &str,
) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("cookie", cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(form.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

const ORG_PAGE: &str = "/orgs/testorg/settings/visibility";
const PROJECT_PAGE: &str = "/orgs/testorg/projects/widget/settings/visibility";

#[tokio::test]
async fn org_page_shows_the_setting_and_an_admin_form() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", None, true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let (status, html) = get(build_router(state), ORG_PAGE, &cookie).await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Private"), "current state is shown");
    assert!(
        html.contains("read-only"),
        "public is explained as read-only"
    );
    assert!(
        html.contains(r#"action="/orgs/testorg/settings/visibility""#),
        "an admin gets the form"
    );
    assert!(
        html.contains(r#"value="private" checked"#),
        "the current value is preselected"
    );
}

#[tokio::test]
async fn org_page_is_read_only_when_forest_says_the_caller_cannot_change_it() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("public", None, false), &calls),
    );
    let cookie = create_test_session_member(&sessions).await;

    let (status, html) = get(build_router(state), ORG_PAGE, &cookie).await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Public"));
    assert!(
        !html.contains(r#"action="/orgs/testorg/settings/visibility""#),
        "no form without the right to change it"
    );
    assert!(html.contains("Only an organisation admin can change"));
}

#[tokio::test]
async fn admin_saves_org_visibility_and_is_sent_back_to_the_page() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", None, true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let response = post(
        build_router(state),
        ORG_PAGE,
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), ORG_PAGE);
    assert_eq!(*calls.lock().unwrap(), vec!["org:testorg=public"]);
}

#[tokio::test]
async fn a_member_cannot_post_org_visibility() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", None, false), &calls),
    );
    let cookie = create_test_session_member(&sessions).await;

    let response = post(
        build_router(state),
        ORG_PAGE,
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(calls.lock().unwrap().is_empty(), "nothing reaches Forest");
}

#[tokio::test]
async fn org_visibility_rejects_bad_csrf_and_unknown_values() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", None, true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let response = post(
        build_router(state.clone()),
        ORG_PAGE,
        &cookie,
        "visibility=public&_csrf=wrong",
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = post(
        build_router(state),
        ORG_PAGE,
        &cookie,
        "visibility=internal&_csrf=test-csrf",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_non_member_gets_no_org_visibility_page() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", None, true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let (status, _) = get(
        build_router(state.clone()),
        "/orgs/otherorg/settings/visibility",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let response = post(
        build_router(state),
        "/orgs/otherorg/settings/visibility",
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_forest_refusal_is_shown_on_the_page() {
    let calls = Arc::new(Mutex::new(vec![]));
    let mock = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_visibility_result: Some(Ok(vis("private", None, true))),
        set_visibility_result: Some(Err(PlatformError::PermissionDenied(
            "changing visibility needs an organisation admin, not an app token".into(),
        ))),
        visibility_calls: Some(calls.clone()),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), mock);
    let cookie = create_test_session(&sessions).await;

    let response = post(
        build_router(state),
        ORG_PAGE,
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let html = body_to_string(response.into_body()).await;
    assert!(html.contains("changing visibility needs an organisation admin"));
}

#[tokio::test]
async fn project_page_shows_own_and_effective_state() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", Some("public"), true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let (status, html) = get(build_router(state), PROJECT_PAGE, &cookie).await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("read-only"));
    assert!(html.contains(r#"value="public" checked"#));
    assert!(html.contains(r#"action="/orgs/testorg/projects/widget/settings/visibility""#));
    assert!(!html.contains("through the organisation"));
}

#[tokio::test]
async fn project_page_says_a_public_organisation_overrides_a_private_component() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("public", Some("private"), true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let (status, html) = get(build_router(state), PROJECT_PAGE, &cookie).await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("through the organisation"));
    assert!(
        html.contains("does not make it private"),
        "says that the component's own private setting does not override the org"
    );
    assert!(html.contains(r#"href="/orgs/testorg/settings/visibility""#));
}

#[tokio::test]
async fn project_page_is_read_only_for_a_member() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", Some("private"), false), &calls),
    );
    let cookie = create_test_session_member(&sessions).await;

    let (status, html) = get(build_router(state), PROJECT_PAGE, &cookie).await;

    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains(r#"action="/orgs/testorg/projects/widget/settings/visibility""#));
    assert!(html.contains("Only an organisation admin can change"));
}

#[tokio::test]
async fn admin_saves_project_visibility() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", Some("private"), true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let response = post(
        build_router(state),
        PROJECT_PAGE,
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), PROJECT_PAGE);
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["project:testorg/widget=public"]
    );
}

#[tokio::test]
async fn a_member_cannot_post_project_visibility() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", Some("private"), false), &calls),
    );
    let cookie = create_test_session_member(&sessions).await;

    let response = post(
        build_router(state),
        PROJECT_PAGE,
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn settings_navigation_links_to_the_visibility_pages() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", Some("private"), true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    let (_, html) = get(
        build_router(state.clone()),
        "/orgs/testorg/settings",
        &cookie,
    )
    .await;
    assert!(html.contains(r#"href="/orgs/testorg/settings/visibility""#));

    let (_, html) = get(build_router(state), PROJECT_PAGE, &cookie).await;
    assert!(html.contains(r#"href="/orgs/testorg/projects/widget/settings/visibility""#));
}

#[tokio::test]
async fn project_page_links_its_visibility_badge_to_the_settings() {
    use forage_core::registry::{ComponentDetail, ComponentSummary, ToolShape};

    let mock = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(ComponentDetail {
            summary: ComponentSummary {
                organisation: "testorg".into(),
                name: "widget".into(),
                latest_version: "1.0.0".into(),
                kind: "binary".into(),
                description: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
                version_count: 1,
                contracts: vec![],
                visibility: "public".into(),
                shape: ToolShape::Component,
                tool: None,
                methods: vec![],
                upstream_host: String::new(),
            },
            versions: vec![],
            readme: String::new(),
            manifest_json: String::new(),
            owners: vec![],
        })),
        ..Default::default()
    });
    let (state, sessions) = test_state_with_registry(MockForestClient::new(), mock, registry);
    let cookie = create_test_session(&sessions).await;

    let (status, html) = get(
        build_router(state),
        "/orgs/testorg/projects/widget",
        &cookie,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"href="/orgs/testorg/projects/widget/settings/visibility""#));
}

#[tokio::test]
async fn a_forest_refusal_to_read_is_a_403_or_404_not_a_500() {
    for (error, expected) in [
        (
            PlatformError::PermissionDenied("not a member of this organisation".into()),
            StatusCode::FORBIDDEN,
        ),
        (
            PlatformError::NotFound("project not found".into()),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let mock = MockPlatformClient::with_behavior(MockPlatformBehavior {
            get_visibility_result: Some(Err(error)),
            ..Default::default()
        });
        let (state, sessions) = test_state_with(MockForestClient::new(), mock);
        let cookie = create_test_session(&sessions).await;

        let (status, _) = get(build_router(state.clone()), ORG_PAGE, &cookie).await;
        assert_eq!(status, expected);
        let (status, _) = get(build_router(state), PROJECT_PAGE, &cookie).await;
        assert_eq!(status, expected);
    }
}

#[tokio::test]
async fn a_member_post_is_refused_even_when_forest_would_show_the_form() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("private", Some("private"), true), &calls),
    );
    let cookie = create_test_session_member(&sessions).await;

    for uri in [ORG_PAGE, PROJECT_PAGE] {
        let response = post(
            build_router(state.clone()),
            uri,
            &cookie,
            "visibility=public&_csrf=test-csrf",
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_forest_error_message_is_escaped() {
    let mock = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_visibility_result: Some(Ok(vis("private", None, true))),
        set_visibility_result: Some(Err(PlatformError::InvalidArgument(
            "<script>alert(1)</script>".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), mock);
    let cookie = create_test_session(&sessions).await;

    let response = post(
        build_router(state),
        ORG_PAGE,
        &cookie,
        "visibility=public&_csrf=test-csrf",
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = body_to_string(response.into_body()).await;
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(html.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn the_pages_say_public_components_are_visible_without_signing_in() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        platform(vis("public", Some("private"), true), &calls),
    );
    let cookie = create_test_session(&sessions).await;

    for uri in [ORG_PAGE, PROJECT_PAGE] {
        let (_, html) = get(build_router(state.clone()), uri, &cookie).await;
        assert!(
            html.contains("not signed in") || html.contains("without signing in"),
            "{uri}"
        );
    }
}
