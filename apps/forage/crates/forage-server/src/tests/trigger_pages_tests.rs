use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{Environment, ReleasePipeline, Trigger, UpdateTriggerInput};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const ID: &str = "11111111-2222-3333-4444-555555555555";

fn trigger() -> Trigger {
    Trigger {
        id: ID.into(),
        name: "main-to-dev".into(),
        enabled: true,
        branch_pattern: Some("main".into()),
        title_pattern: None,
        author_pattern: None,
        commit_message_pattern: None,
        source_type_pattern: None,
        target_environments: vec!["dev".into()],
        target_destinations: vec!["dev-eu".into(), "dev-us".into()],
        force_release: false,
        use_pipeline: true,
        created_at: "2026-03-08T00:00:00Z".into(),
        updated_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn environment(name: &str) -> Environment {
    Environment {
        id: format!("env-{name}"),
        organisation: "testorg".into(),
        name: name.into(),
        description: None,
        sort_order: 0,
        created_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn pipeline() -> ReleasePipeline {
    ReleasePipeline {
        id: "p1".into(),
        name: "standard".into(),
        enabled: true,
        stages: vec![],
        created_at: "2026-03-08T00:00:00Z".into(),
        updated_at: "2026-03-08T00:00:00Z".into(),
    }
}

struct Setup {
    triggers: Vec<Trigger>,
    pipelines: Vec<ReleasePipeline>,
}

impl Default for Setup {
    fn default() -> Self {
        Self { triggers: vec![trigger()], pipelines: vec![pipeline()] }
    }
}

async fn app(setup: Setup) -> (axum::Router, String, Arc<Mutex<Vec<UpdateTriggerInput>>>) {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_triggers_result: Some(Ok(setup.triggers)),
        list_environments_result: Some(Ok(vec![environment("dev"), environment("prod")])),
        list_release_pipelines_result: Some(Ok(setup.pipelines)),
        update_trigger_inputs: Some(inputs.clone()),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    (build_router(state), cookie, inputs)
}

async fn get(app: axum::Router, cookie: &str, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::builder().uri(uri).header("cookie", cookie).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn post(app: axum::Router, cookie: &str, uri: &str, form: &str) -> StatusCode {
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
    .status()
}

fn main_element(html: &str) -> &str {
    let start = html.find("<main").expect("a <main>");
    let end = html.find("</main>").expect("a </main>") + "</main>".len();
    &html[start..end]
}

fn edit_uri() -> String {
    format!("/orgs/testorg/projects/my-api/settings/triggers/{ID}")
}

#[tokio::test]
async fn saving_a_trigger_keeps_its_target_destinations() {
    let (app, cookie, inputs) = app(Setup::default()).await;
    let status = post(
        app,
        &cookie,
        &edit_uri(),
        "csrf_token=test-csrf&branch_pattern=main&target_environments=dev&use_pipeline_offered=true&use_pipeline=true",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let inputs = inputs.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].target_destinations, vec!["dev-eu", "dev-us"]);
    assert_eq!(inputs[0].target_environments, vec!["dev"]);
}

#[tokio::test]
async fn saving_a_trigger_sends_unset_patterns_as_unset() {
    let (app, cookie, inputs) = app(Setup::default()).await;
    post(
        app,
        &cookie,
        &edit_uri(),
        "csrf_token=test-csrf&branch_pattern=main&title_pattern=&author_pattern=&commit_message_pattern=&source_type_pattern=&target_environments=dev",
    )
    .await;
    let input = &inputs.lock().unwrap()[0];
    assert_eq!(input.branch_pattern.as_deref(), Some("main"));
    assert_eq!(input.title_pattern, None);
    assert_eq!(input.author_pattern, None);
    assert_eq!(input.commit_message_pattern, None);
    assert_eq!(input.source_type_pattern, None);
}

#[tokio::test]
async fn saving_a_trigger_the_form_offered_no_pipeline_choice_for_leaves_use_pipeline_alone() {
    let (app, cookie, inputs) = app(Setup { pipelines: vec![], ..Default::default() }).await;
    post(app, &cookie, &edit_uri(), "csrf_token=test-csrf&branch_pattern=main&target_environments=dev").await;
    assert_eq!(inputs.lock().unwrap()[0].use_pipeline, None);
}

#[tokio::test]
async fn unticking_use_pipeline_turns_it_off() {
    let (app, cookie, inputs) = app(Setup::default()).await;
    post(
        app,
        &cookie,
        &edit_uri(),
        "csrf_token=test-csrf&branch_pattern=main&target_environments=dev&use_pipeline_offered=true",
    )
    .await;
    assert_eq!(inputs.lock().unwrap()[0].use_pipeline, Some(false));
}

#[tokio::test]
async fn the_edit_form_offers_use_pipeline_only_with_pipelines() {
    let (app_with, cookie, _) = app(Setup::default()).await;
    let (_, html) = get(app_with, &cookie, &edit_uri()).await;
    assert!(html.contains(r#"name="use_pipeline_offered""#), "{html}");
    assert!(html.contains(r#"name="use_pipeline" value="true" checked"#), "{html}");

    let (app_without, cookie, _) = app(Setup { pipelines: vec![], ..Default::default() }).await;
    let (_, html) = get(app_without, &cookie, &edit_uri()).await;
    assert!(!html.contains("use_pipeline"), "{html}");
}

#[tokio::test]
async fn the_edit_page_renders_unset_patterns_empty_and_checks_current_targets() {
    let (app, cookie, _) = app(Setup::default()).await;
    let (status, html) = get(app, &cookie, &edit_uri()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains(r#"value="none""#));
    assert!(html.contains(r#"name="branch_pattern" value="main""#), "{html}");
    assert!(html.contains(r#"name="title_pattern" value="""#), "{html}");
    assert!(html.contains(r#"name="target_environments" value="dev" checked"#), "{html}");
    assert!(html.contains(r#"name="target_environments" value="prod" class"#), "{html}");
    insta::assert_snapshot!(main_element(&html));
}

#[tokio::test]
async fn the_triggers_page_lists_triggers_and_their_actions() {
    let mut disabled = trigger();
    disabled.id = "t2".into();
    disabled.name = "release-\"quoted\"".into();
    disabled.enabled = false;
    disabled.use_pipeline = false;
    disabled.force_release = true;
    disabled.branch_pattern = None;
    disabled.title_pattern = Some("^Release".into());
    let (app, cookie, _) = app(Setup { triggers: vec![trigger(), disabled], ..Default::default() }).await;
    let (status, html) = get(app, &cookie, "/orgs/testorg/projects/my-api/settings/triggers").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("onsubmit"), "{html}");
    assert!(!html.contains("<script>"), "{html}");
    assert!(html.contains(r#"data-confirm="Delete trigger &quot;release-&quot;quoted&quot;&quot;?""#), "{html}");
    assert!(html.contains("/static/js/trigger-form.js?v="), "{html}");
    insta::assert_snapshot!(main_element(&html));
}

#[tokio::test]
async fn the_triggers_page_without_triggers_invites_an_admin_to_create_one() {
    let (app, cookie, _) = app(Setup { triggers: vec![], ..Default::default() }).await;
    let (_, html) = get(app, &cookie, "/orgs/testorg/projects/my-api/settings/triggers").await;
    assert!(html.contains("No triggers configured."));
    assert!(html.contains("Create one below to automatically release matching artifacts."));
    assert!(html.contains(r#"id="trigger-form""#));
}
