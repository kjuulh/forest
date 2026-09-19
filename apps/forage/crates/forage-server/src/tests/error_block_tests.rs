//! The failure reason on a release page is a block, not a row cell.
//!
//! This exists because the bug it guards against is invisible in a template
//! diff: a `<span>` inside the stage's flex row looked fine for "deploy
//! failed" and only fell apart once a real chained error arrived — several
//! hundred characters that wrapped inside the row, tripled its height and
//! pushed the badge and timestamp around. The assertion is therefore
//! structural: the error text must not be inside the row element.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    Artifact, ArtifactContext, DeploymentStates, DestinationState, PipelineRunStageState,
    ReleaseIntentState,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

/// The real message from the failure this was built for: long, a repeated
/// ~100-character URL, and a chain whose *last* clause is the actionable one.
const LONG_ERROR: &str = "provider reported failure: compare the candidate configuration at https://commission-next.finance.understory.sh/admin/commission/compare?format=json&candidate_ref=main: https://commission-next.finance.understory.sh/admin/commission/compare?format=json&candidate_ref=main returned 500 Internal Server Error: comparison failed: source \"sdr_revenue\" row 0 column \"revenue_bloom_eur\" must be an exact decimal string";

const SHORT_ERROR: &str = "upstream stage failed";

fn failed_pipeline_fixture() -> MockPlatformBehavior {
    MockPlatformBehavior {
        get_artifact_by_slug_result: Some(Ok(Artifact {
            artifact_id: "art-fail".into(),
            slug: "my-api-fail".into(),
            context: ArtifactContext {
                title: "Adopt the candidate commission configuration".into(),
                description: None,
                web: None,
                pr: None,
            },
            source: None,
            git_ref: None,
            destinations: vec![],
            created_at: "2026-03-07T12:00:00Z".into(),
        })),
        get_destination_states_result: Some(Ok(DeploymentStates {
            destinations: vec![
                DestinationState {
                    destination_id: "d-dev".into(),
                    destination_name: "data-dev/commission".into(),
                    environment: "data-dev".into(),
                    release_id: Some("rel-1".into()),
                    artifact_id: Some("art-fail".into()),
                    status: Some("SUCCEEDED".into()),
                    error_message: None,
                    queued_at: None,
                    completed_at: Some("2026-03-07T12:01:00Z".into()),
                    queue_position: None,
                    started_at: Some("2026-03-07T12:00:30Z".into()),
                },
                DestinationState {
                    destination_id: "d-prod".into(),
                    destination_name: "data-prod/commission".into(),
                    environment: "data-prod".into(),
                    release_id: Some("rel-2".into()),
                    artifact_id: Some("art-fail".into()),
                    status: Some("FAILED".into()),
                    error_message: Some(LONG_ERROR.into()),
                    queued_at: None,
                    completed_at: Some("2026-03-07T12:04:00Z".into()),
                    queue_position: None,
                    started_at: Some("2026-03-07T12:02:00Z".into()),
                },
            ],
        })),
        get_release_intent_states_result: Some(Ok(vec![ReleaseIntentState {
            release_intent_id: "intent-fail".into(),
            artifact_id: "art-fail".into(),
            project: "my-api".into(),
            created_at: "2026-03-07T12:00:00Z".into(),
            stages: vec![
                PipelineRunStageState {
                    stage_id: "deploy-dev".into(),
                    stage_type: "deploy".into(),
                    status: "SUCCEEDED".into(),
                    environment: Some("data-dev".into()),
                    started_at: Some("2026-03-07T12:00:30Z".into()),
                    completed_at: Some("2026-03-07T12:01:00Z".into()),
                    ..Default::default()
                },
                PipelineRunStageState {
                    stage_id: "deploy-prod".into(),
                    depends_on: vec!["deploy-dev".into()],
                    stage_type: "deploy".into(),
                    status: "FAILED".into(),
                    environment: Some("data-prod".into()),
                    started_at: Some("2026-03-07T12:02:00Z".into()),
                    completed_at: Some("2026-03-07T12:04:00Z".into()),
                    error_message: Some(LONG_ERROR.into()),
                    ..Default::default()
                },
                PipelineRunStageState {
                    stage_id: "deploy-eu".into(),
                    depends_on: vec!["deploy-prod".into()],
                    stage_type: "deploy".into(),
                    status: "CANCELLED".into(),
                    environment: Some("data-prod-eu".into()),
                    error_message: Some(SHORT_ERROR.into()),
                    ..Default::default()
                },
            ],
            steps: vec![],
        }])),
        ..Default::default()
    }
}

async fn render_page(slug: &str, behavior: MockPlatformBehavior) -> String {
    let platform = MockPlatformClient::with_behavior(behavior);
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/orgs/testorg/projects/my-api/releases/{slug}"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// Everything between a `flex items-center` opening tag and its matching
/// `</div>`, nesting-aware — i.e. the stage/destination rows as the browser
/// sees them.
fn flex_rows(html: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut search = 0usize;
    while let Some(offset) = html[search..].find("flex items-center") {
        let marker = search + offset;
        search = marker + 1;
        let Some(tag_start) = html[..marker].rfind("<div") else {
            continue;
        };
        // Walk forward closing one `<div>` for every `</div>`, so a nested
        // element cannot end the row early.
        let mut depth = 1i32;
        let mut cursor = tag_start + 4;
        let row_end = loop {
            let next_open = html[cursor..].find("<div").map(|p| cursor + p);
            let next_close = html[cursor..].find("</div>").map(|p| cursor + p);
            match (next_open, next_close) {
                (Some(o), Some(c)) if o < c => {
                    depth += 1;
                    cursor = o + 4;
                }
                (_, Some(c)) => {
                    depth -= 1;
                    cursor = c + 6;
                    if depth == 0 {
                        break cursor;
                    }
                }
                _ => break html.len(),
            }
        };
        rows.push(html[tag_start..row_end].to_string());
    }
    rows
}

#[tokio::test]
async fn a_stage_error_is_not_inside_the_stage_row() {
    let html = render_page("my-api-fail", failed_pipeline_fixture()).await;

    assert!(
        html.contains("must be an exact decimal string"),
        "the actionable tail of the chain has to survive to the page"
    );
    for row in flex_rows(&html) {
        assert!(
            !row.contains("comparison failed"),
            "an error belongs beneath the row, never inside it:\n{row}"
        );
    }
}

#[tokio::test]
async fn a_long_error_is_a_panel_and_a_short_one_is_not() {
    let html = render_page("my-api-fail", failed_pipeline_fixture()).await;

    let panel = html
        .match_indices("bg-red-50 border border-red-200")
        .map(|(i, _)| {
            &html[i..html[i..]
                .find("</div>")
                .map(|e| i + e)
                .unwrap_or(html.len())]
        })
        .find(|block| block.contains("comparison failed"))
        .expect("a long error gets the panel");
    assert!(
        panel.contains("break-words") && panel.contains("whitespace-pre-wrap"),
        "an unbroken URL has to wrap rather than overflow: {panel}"
    );

    // The short reason stays a plain line -- no border, no tint.
    let short_start = html.find(SHORT_ERROR).expect("short reason rendered");
    let line_start = html[..short_start].rfind('<').unwrap();
    let line = &html[line_start..short_start];
    assert!(
        !line.contains("border") && !line.contains("bg-red-50"),
        "four words do not need a panel: {line}"
    );
}

#[tokio::test]
async fn nothing_is_truncated_and_no_tooltip_stands_in_for_the_text() {
    let html = render_page("my-api-fail", failed_pipeline_fixture()).await;

    assert!(
        !html.contains(&format!("title=\"{LONG_ERROR}\"")),
        "a tooltip is not a substitute for readable, selectable text"
    );
    // Both the stage and the destination row carry the same message; both
    // must render it whole.
    assert_eq!(
        html.matches("must be an exact decimal string").count(),
        2,
        "the stage and the destination each show the reason in full"
    );
}

fn clean_pipeline_fixture() -> MockPlatformBehavior {
    let mut behavior = failed_pipeline_fixture();
    if let Some(Ok(states)) = behavior.get_release_intent_states_result.as_mut() {
        for stage in &mut states[0].stages {
            stage.status = "SUCCEEDED".into();
            stage.error_message = None;
        }
    }
    if let Some(Ok(states)) = behavior.get_destination_states_result.as_mut() {
        for dest in &mut states.destinations {
            dest.status = Some("SUCCEEDED".into());
            dest.error_message = None;
        }
    }
    behavior
}

#[tokio::test]
async fn a_clean_pipeline_renders_no_error_furniture() {
    let html = render_page("my-api-fail", clean_pipeline_fixture()).await;

    assert!(!html.contains("bg-red-50"), "no panel on a clean release");
    assert!(!html.contains("text-red-600"), "nothing red to report");
}
