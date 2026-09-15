//! The project settings shell — `/orgs/{org}/projects/{project}/settings`.
//!
//! Triggers, Policies and Pipelines were three standalone pages: no shared
//! sub-nav, no project header, and no way back to the project once you were
//! on one. These cover the chrome that fixes that, and the redirects that
//! keep the old URLs alive.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{Policy, PolicyConfig, Trigger};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const PROJECT: &str = "/orgs/testorg/projects/my-api";
const TRIGGER_ID: &str = "11111111-2222-3333-4444-555555555555";
const POLICY_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

fn a_trigger() -> Trigger {
    Trigger {
        id: TRIGGER_ID.into(),
        name: "main-to-staging".into(),
        enabled: true,
        branch_pattern: Some("main".into()),
        title_pattern: None,
        author_pattern: None,
        commit_message_pattern: None,
        source_type_pattern: None,
        target_environments: vec!["staging".into()],
        target_destinations: vec![],
        force_release: false,
        use_pipeline: false,
        created_at: "2026-03-08T00:00:00Z".into(),
        updated_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn a_policy() -> Policy {
    Policy {
        id: POLICY_ID.into(),
        name: "staging-soak-30m".into(),
        enabled: true,
        policy_type: "soak_time".into(),
        config: PolicyConfig::SoakTime {
            source_environment: "staging".into(),
            target_environment: "prod".into(),
            duration_seconds: 1800,
        },
        created_at: "2026-03-08T00:00:00Z".into(),
        updated_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn populated_state() -> (
    crate::state::AppState,
    std::sync::Arc<forage_core::session::InMemorySessionStore>,
) {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_triggers_result: Some(Ok(vec![a_trigger()])),
        list_policies_result: Some(Ok(vec![a_policy()])),
        ..Default::default()
    });
    test_state_with(MockForestClient::new(), platform)
}

async fn get(uri: &str) -> (StatusCode, String, Option<String>) {
    let (state, sessions) = populated_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string());
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap(), location)
}

// ─── The shell ──────────────────────────────────────────────────────

/// `/settings` has no content of its own — it opens on the first section
/// rather than a hub page nobody would read.
#[tokio::test]
async fn settings_root_opens_on_triggers() {
    let (status, _, location) = get(&format!("{PROJECT}/settings")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.unwrap(), format!("{PROJECT}/settings/triggers"));
}

/// The reported gap: every settings page must offer a way back. Each
/// section carries the full breadcrumb and an explicit Back link.
#[tokio::test]
async fn every_section_has_a_breadcrumb_and_a_way_back() {
    for section in ["triggers", "policies", "pipelines"] {
        let (status, html, _) = get(&format!("{PROJECT}/settings/{section}")).await;
        assert_eq!(status, StatusCode::OK, "{section}");
        assert!(html.contains("aria-label=\"Breadcrumb\""), "{section}");
        // …and the breadcrumb's own links out, plus the Back affordance.
        assert!(html.contains(&format!("href=\"{PROJECT}\"")), "{section}");
        assert!(html.contains("Back to my-api"), "{section}");
    }
}

/// One click between sections, from any section — no dead ends.
#[tokio::test]
async fn every_section_links_to_every_other_section() {
    for section in ["triggers", "policies", "pipelines"] {
        let (_, html, _) = get(&format!("{PROJECT}/settings/{section}")).await;
        for other in ["triggers", "policies", "pipelines"] {
            assert!(
                html.contains(&format!("href=\"{PROJECT}/settings/{other}\"")),
                "{section} page is missing a link to {other}"
            );
        }
        // The section you are on is marked, not just styled.
        assert!(
            html.contains(&format!(
                "href=\"{PROJECT}/settings/{section}\" aria-current=\"page\""
            )),
            "{section} is not marked current"
        );
    }
}

/// The project header, so you always know what you are configuring — and
/// the project-level tab row, which these pages used to lose entirely
/// (they passed no `project_name`, so base.html.jinja fell back to the
/// org tabs and the breadcrumb read "Select project").
#[tokio::test]
async fn sections_carry_the_project_identity() {
    for section in ["triggers", "policies", "pipelines"] {
        let (_, html, _) = get(&format!("{PROJECT}/settings/{section}")).await;
        assert!(html.contains("my-api"), "{section}");
        assert!(!html.contains("Select project"), "{section}");
        // Project tabs, with Settings marked active.
        assert!(
            html.contains(&format!("href=\"{PROJECT}/releases\"")),
            "{section}"
        );
        assert!(
            html.contains(&format!("href=\"{PROJECT}/settings\"")),
            "{section}"
        );
    }
}

/// An edit form is a level below its section, so Back means the list you
/// came from rather than the project.
#[tokio::test]
async fn edit_pages_go_back_to_their_section() {
    let (status, html, _) = get(&format!("{PROJECT}/settings/triggers/{TRIGGER_ID}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Back to Triggers"));
    assert!(html.contains(&format!("href=\"{PROJECT}/settings/triggers\"")));
    assert!(html.contains("main-to-staging"));

    let (status, html, _) = get(&format!("{PROJECT}/settings/policies/{POLICY_ID}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Back to Policies"));
    assert!(html.contains(&format!("href=\"{PROJECT}/settings/policies\"")));
}

/// Behaviour unchanged: the rows still carry their own Edit / Disable /
/// Delete actions, addressed by id (forest#259), just at the new prefix.
#[tokio::test]
async fn section_rows_keep_their_actions() {
    let (_, html, _) = get(&format!("{PROJECT}/settings/triggers")).await;
    let base = format!("{PROJECT}/settings/triggers/{TRIGGER_ID}");
    assert!(html.contains(&format!("href=\"{base}\"")));
    assert!(html.contains(&format!("action=\"{base}/toggle\"")));
    assert!(html.contains(&format!("action=\"{base}/delete\"")));
}

// ─── Old URLs ───────────────────────────────────────────────────────

/// Bookmarks and old links keep working.
#[tokio::test]
async fn legacy_section_urls_redirect_into_the_shell() {
    for section in ["triggers", "policies", "pipelines"] {
        let (status, _, location) = get(&format!("{PROJECT}/{section}")).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{section}");
        assert_eq!(
            location.unwrap(),
            format!("{PROJECT}/settings/{section}"),
            "{section}"
        );
    }
}

#[tokio::test]
async fn legacy_edit_urls_redirect_into_the_shell() {
    let (status, _, location) = get(&format!("{PROJECT}/triggers/{TRIGGER_ID}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location.unwrap(),
        format!("{PROJECT}/settings/triggers/{TRIGGER_ID}")
    );

    let (status, _, location) = get(&format!("{PROJECT}/policies/{POLICY_ID}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location.unwrap(),
        format!("{PROJECT}/settings/policies/{POLICY_ID}")
    );
}

/// A GET redirect would silently drop a POST body, so the legacy paths
/// keep their original submit handlers rather than redirecting: anything
/// still posting to the old URL creates the trigger, it doesn't 405.
#[tokio::test]
async fn posting_to_a_legacy_section_url_still_works() {
    let (state, sessions) = populated_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{PROJECT}/triggers"))
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "csrf_token=test-csrf&name=from-legacy-url&branch_pattern=main&target_environments=staging",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        format!("{PROJECT}/settings/triggers").as_str()
    );
}
