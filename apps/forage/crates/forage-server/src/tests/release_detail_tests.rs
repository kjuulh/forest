use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    ApprovalDecisionEntry, ApprovalState, Artifact, ArtifactContext, ArtifactDestination,
    ArtifactRef, ArtifactSource, DeploymentStates, DestinationState, Environment,
    PipelineRunStageState, PlanOutput, PolicyEvaluation, ReleaseIntentState, ReleasePipeline,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const PAGE: &str = "/orgs/testorg/projects/my-api/releases/my-api-rel";

fn artifact() -> Artifact {
    Artifact {
        artifact_id: "art-rel".into(),
        slug: "my-api-rel".into(),
        context: ArtifactContext {
            title: "Release the billing service".into(),
            description: Some("Moves billing to the new ledger".into()),
            web: Some("https://example.com/billing".into()),
            pr: Some("https://github.com/acme/billing/pull/7".into()),
        },
        source: Some(ArtifactSource {
            user: Some("ci-bot".into()),
            email: Some("ci@example.com".into()),
            source_type: Some("github_actions".into()),
            run_url: Some("https://github.com/acme/billing/actions/runs/1".into()),
        }),
        git_ref: Some(ArtifactRef {
            commit_sha: "0123456789abcdef".into(),
            branch: Some("main".into()),
            commit_message: Some("feat: ledger\n\nLonger body".into()),
            version: Some("v3.1.0".into()),
            repo_url: Some("https://github.com/acme/billing".into()),
        }),
        destinations: vec![],
        created_at: "2026-03-07T12:00:00Z".into(),
    }
}

fn stage(id: &str, kind: &str, status: &str, env: Option<&str>) -> PipelineRunStageState {
    PipelineRunStageState {
        stage_id: id.into(),
        stage_type: kind.into(),
        status: status.into(),
        environment: env.map(Into::into),
        ..Default::default()
    }
}

fn destination(name: &str, env: &str, status: &str) -> DestinationState {
    DestinationState {
        destination_id: format!("d-{name}"),
        destination_name: name.into(),
        environment: env.into(),
        release_id: Some(format!("rel-{name}")),
        artifact_id: Some("art-rel".into()),
        status: Some(status.into()),
        error_message: None,
        queued_at: Some("2026-03-07T12:00:00Z".into()),
        completed_at: None,
        queue_position: None,
        started_at: None,
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

fn decision(username: &str, decision: &str, comment: Option<&str>) -> ApprovalDecisionEntry {
    ApprovalDecisionEntry {
        user_id: format!("u-{username}"),
        username: username.into(),
        decision: decision.into(),
        decided_at: "2026-03-07T12:10:00Z".into(),
        comment: comment.map(Into::into),
    }
}

fn behaviour() -> MockPlatformBehavior {
    let deploy = PipelineRunStageState {
        started_at: Some("2026-03-07T12:00:30Z".into()),
        ..stage("deploy-staging", "deploy", "SUCCEEDED", Some("staging"))
    };
    let wait = PipelineRunStageState {
        depends_on: vec!["deploy-staging".into()],
        duration_seconds: Some(30),
        ..stage("soak", "wait", "SUCCEEDED", None)
    };
    let plan = PipelineRunStageState {
        depends_on: vec!["soak".into()],
        approval_status: Some("AWAITINGAPPROVAL".into()),
        ..stage("plan-prod", "plan", "RUNNING", Some("prod"))
    };
    let mut queued = destination("prod-eu", "prod", "QUEUED");
    queued.queue_position = Some(2);
    let mut deployed = destination("staging-eu", "staging", "SUCCEEDED");
    deployed.completed_at = Some("2026-03-07T12:01:00Z".into());
    MockPlatformBehavior {
        get_artifact_by_slug_result: Some(Ok(artifact())),
        get_release_intent_states_result: Some(Ok(vec![ReleaseIntentState {
            release_intent_id: "intent-rel".into(),
            artifact_id: "art-rel".into(),
            project: "my-api".into(),
            created_at: "2026-03-07T12:00:00Z".into(),
            stages: vec![deploy, wait, plan],
            steps: vec![],
        }])),
        get_destination_states_result: Some(Ok(DeploymentStates {
            destinations: vec![deployed, queued],
        })),
        get_plan_output_result: Some(Ok(PlanOutput {
            plan_output: "+ ledger_table <new>".into(),
            status: "SUCCEEDED".into(),
            outputs: vec![],
        })),
        evaluate_policies_result: Some(Ok(vec![
            PolicyEvaluation {
                policy_name: "two-person-rule".into(),
                policy_type: "approval".into(),
                passed: false,
                reason: "1 of 2 approvals".into(),
                approval_state: Some(ApprovalState {
                    required_approvals: 2,
                    current_approvals: 1,
                    decisions: vec![
                        decision("alice", "approved", Some("looks good")),
                        decision("bob", "rejected", None),
                    ],
                }),
            },
            PolicyEvaluation {
                policy_name: "soak-an-hour".into(),
                policy_type: "soak_time".into(),
                passed: true,
                reason: "soaked".into(),
                approval_state: None,
            },
        ])),
        list_environments_result: Some(Ok(vec![environment("staging"), environment("prod")])),
        list_release_pipelines_result: Some(Ok(vec![ReleasePipeline {
            id: "p1".into(),
            name: "standard".into(),
            enabled: true,
            stages: vec![],
            created_at: "2026-03-08T00:00:00Z".into(),
            updated_at: "2026-03-08T00:00:00Z".into(),
        }])),
        get_artifact_spec_result: Some(Ok("project: \"billing\" <&>".into())),
        ..Default::default()
    }
}

async fn page_as(behavior: MockPlatformBehavior, member: bool) -> String {
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(behavior),
    );
    let cookie = if member {
        create_test_session_member(&sessions).await
    } else {
        create_test_session(&sessions).await
    };
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri(PAGE)
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

async fn page(behavior: MockPlatformBehavior) -> String {
    page_as(behavior, false).await
}

fn main_element(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    html[start..end].to_string()
}

fn form<'a>(html: &'a str, action: &str) -> Vec<&'a str> {
    html.split("<form ")
        .skip(1)
        .filter(|f| f.starts_with(&format!(r#"method="post" action="{action}""#)))
        .map(|f| &f[..f.find("</form>").unwrap()])
        .collect()
}

#[tokio::test]
async fn the_release_page_renders_without_inline_script_or_handlers() {
    let html = page(behaviour()).await;
    assert!(!html.contains("<script>"), "no inline scripts");
    for attr in [" onsubmit=", " onclick=", " onchange=", " style="] {
        assert!(!html.contains(attr), "{attr} found");
    }
    assert!(
        html.contains("/static/js/components/forage-components.js?v="),
        "the islands bundle is loaded"
    );
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects/my-api/releases" aria-current="page""#),
        "the Releases tab is current"
    );
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&main_element(&html), "?v=HASH")
        .into_owned();
    let main = regex::Regex::new(r#"(<time datetime="[^"]+"[^>]*>)[^<]*</time>"#)
        .unwrap()
        .replace_all(&main, "${1}AGO</time>")
        .into_owned();
    insta::assert_snapshot!(main);
}

#[tokio::test]
async fn the_header_shows_the_first_line_and_keeps_the_whole_message_as_its_title() {
    let html = page(behaviour()).await;
    assert!(
        html.contains(
            r#"title="feat: ledger

Longer body">feat: ledger…</h1>"#
        ),
        "{html}"
    );
    assert!(html.contains(">v3.1.0</span>"));
    assert!(html.contains("01234567</span>") && !html.contains("0123456789"));
    assert!(
        html.contains("github actions</span>"),
        "underscores read as spaces"
    );
    assert!(html.contains(r#"href="/users/ci-bot""#));
    assert!(html.contains(r#"title="Moves billing to the new ledger""#));
}

#[tokio::test]
async fn an_admin_can_deploy_the_release_through_the_pipeline() {
    let html = page(behaviour()).await;
    let forms = form(&html, "/orgs/testorg/projects/my-api/deploy");
    assert_eq!(forms.len(), 1, "{html}");
    let deploy = forms[0];
    assert!(deploy.contains(r#"<input type="hidden" name="_csrf" value="test-csrf">"#));
    assert!(deploy.contains(r#"name="artifact_id" value="art-rel""#));
    assert!(deploy.contains(r#"name="use_pipeline" value="true" checked"#));
    assert!(deploy.contains(r#"<option value="">All environments (pipeline decides)</option>"#));
    assert!(deploy.contains(r#"<option value="staging">staging</option>"#));
    assert!(deploy.contains(r#"<option value="prod">prod</option>"#));
}

#[tokio::test]
async fn a_member_gets_no_deploy_form_and_no_admin_bypass() {
    let html = page_as(behaviour(), true).await;
    assert!(!html.contains("Deploy this release"));
    assert!(!html.contains("force_bypass"));
    assert!(
        html.contains(">Approve</button>"),
        "members can still approve"
    );
}

#[tokio::test]
async fn a_plan_awaiting_approval_posts_to_the_plan_stage_api_and_returns_here() {
    let html = page(behaviour()).await;
    for action in ["approve", "reject"] {
        let forms = form(
            &html,
            &format!("/api/orgs/testorg/projects/my-api/plan-stages/plan-prod/{action}"),
        );
        assert_eq!(forms.len(), 1, "{action}: {html}");
        let f = forms[0];
        assert!(f.contains(r#"name="csrf_token" value="test-csrf""#));
        assert!(f.contains(r#"name="release_intent_id" value="intent-rel""#));
        assert!(f.contains(
            r#"name="redirect_to" value="/orgs/testorg/projects/my-api/releases/my-api-rel""#
        ));
    }
    assert!(html.contains(
        r#"/plan-stages/plan-prod/reject" class="inline" data-confirm="Reject this plan?""#
    ));
    assert!(html.contains("Awaiting plan approval"));
    assert!(html.contains(r#"<details data-plan-output class="px-4 pb-3" open>"#));
    assert!(html.contains(r#"id="plan-plan-prod""#));
    assert!(
        html.contains("+ ledger_table &lt;new&gt;"),
        "plan output is escaped text"
    );
}

#[tokio::test]
async fn approving_a_plan_from_the_page_redirects_back_to_the_release() {
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(behaviour()),
    );
    let cookie = create_test_session(&sessions).await;
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orgs/testorg/projects/my-api/plan-stages/plan-prod/approve")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "csrf_token=test-csrf&release_intent_id=intent-rel&redirect_to=%2Forgs%2Ftestorg%2Fprojects%2Fmy-api%2Freleases%2Fmy-api-rel",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], PAGE);
}

#[tokio::test]
async fn a_pending_approval_offers_approve_bypass_and_reject_for_its_environment() {
    let html = page(behaviour()).await;
    let approve = form(
        &html,
        "/orgs/testorg/projects/my-api/releases/my-api-rel/approve",
    );
    let approve: Vec<&str> = approve
        .into_iter()
        .filter(|f| f.contains(r#"name="target_environment" value="prod""#))
        .collect();
    assert_eq!(approve.len(), 2, "approve and admin bypass for prod");
    for f in &approve {
        assert!(f.contains(r#"name="csrf_token" value="test-csrf""#));
        assert!(f.contains(r#"name="release_intent_id" value="intent-rel""#));
        assert!(
            f.contains(r#"name="target_environment" value="prod""#),
            "{f}"
        );
    }
    assert!(approve[1].contains(r#"name="force_bypass" value="true""#));
    assert!(approve[1].contains(r#"data-confirm="Admin bypass — skip remaining approvals?""#));
    let reject = form(
        &html,
        "/orgs/testorg/projects/my-api/releases/my-api-rel/reject",
    );
    assert_eq!(
        reject.len(),
        2,
        "one per environment the pipeline deploys to"
    );
    assert!(
        reject
            .iter()
            .any(|f| f.contains(r#"name="target_environment" value="staging""#))
    );
    assert!(
        reject
            .iter()
            .any(|f| f.contains(r#"name="target_environment" value="prod""#))
    );

    assert!(html.contains("1/2 approvals"));
    assert!(html.contains(r#"<span class="text-green-700 font-medium">alice</span>"#));
    assert!(html.contains("— looks good"));
    assert!(html.contains(r#"<span class="text-red-700 font-medium">bob</span>"#));
    assert!(html.contains("2/4 passed"));
    assert!(html.contains("2 passed policies"));
    assert!(html.contains(">Approval</span>") && html.contains(">Soak Time</span>"));
}

#[tokio::test]
async fn stages_and_destinations_show_their_status_and_environment() {
    let html = page(behaviour()).await;
    assert!(html.contains(r#"<span class="text-sm text-gray-700">Deployed to</span>"#));
    assert!(html.contains("Waited 30s"));
    assert!(html.contains("2/3 stages complete"));
    assert!(
        html.contains(r#"bg-yellow-100 text-yellow-800">staging</span>"#),
        "staging badge"
    );
    assert!(
        html.contains(r#"bg-pink-100 text-pink-800">prod</span>"#),
        "prod badge"
    );
    assert!(html.contains(r#"<span class="text-xs text-blue-600">Queued #2</span>"#));
    assert!(html.contains(r#"<span class="text-xs text-green-600">Deployed</span>"#));
    assert!(html.contains(r#"id="dest-prod-eu""#) && html.contains(r#"id="stage-deploy-staging""#));
    assert!(html.contains(r#"<release-logs id="rl-testorg-my-api-my-api-rel" url="/api/orgs/testorg/projects/my-api/releases/my-api-rel/logs"></release-logs>"#));
    assert!(html.contains(r#"<spec-viewer id="spec-testorg-my-api-my-api-rel" content="project: &quot;billing&quot; &lt;&amp;&gt;" filename="forest.cue">"#), "{html}");
}

#[tokio::test]
async fn configured_destinations_are_listed_as_not_deployed_before_anything_ran() {
    let mut behavior = behaviour();
    behavior.get_destination_states_result = Some(Ok(DeploymentStates::default()));
    let mut art = artifact();
    art.destinations = vec![ArtifactDestination {
        name: "prod-us".into(),
        environment: "prod".into(),
        type_organisation: None,
        type_name: None,
        type_version: None,
        status: None,
    }];
    behavior.get_artifact_by_slug_result = Some(Ok(art));
    let html = page(behavior).await;
    assert!(html.contains(r#"<span class="text-gray-700">prod-us</span>"#));
    assert!(html.contains("Not deployed"));
}

#[tokio::test]
async fn a_bare_release_renders_no_empty_sections_and_no_none() {
    let mut behavior = MockPlatformBehavior::default();
    behavior.get_artifact_by_slug_result = Some(Ok(Artifact {
        context: ArtifactContext {
            title: "Bare".into(),
            description: None,
            web: None,
            pr: None,
        },
        source: Some(ArtifactSource {
            user: None,
            email: None,
            source_type: None,
            run_url: None,
        }),
        git_ref: None,
        ..artifact()
    }));
    let html = page(behavior).await;
    let main = main_element(&html);
    let leak =
        regex::Regex::new(r#"(?i)>[^<]*\bnone\b[^<]*<|(value|content|title|href)="none""#).unwrap();
    assert!(!leak.is_match(&main), "{main}");
    for section in [
        "Pipeline</h2>",
        "Policy Requirements",
        "Destinations</h2>",
        "Logs</h2>",
        "Spec</h2>",
        "Details</h2>",
        "Links</h2>",
    ] {
        assert!(!main.contains(section), "{section} should not render");
    }
    assert!(html.contains(r#"<meta name="description" content="">"#));
}

#[tokio::test]
async fn links_with_a_script_scheme_are_not_followed() {
    let mut behavior = behaviour();
    let mut art = artifact();
    art.context.web = Some("javascript:alert(1)".into());
    behavior.get_artifact_by_slug_result = Some(Ok(art));
    let html = page(behavior).await;
    assert!(
        !html.contains("javascript:alert"),
        "unsafe scheme reached the page"
    );
}

async fn decide_plan(action: &str, redirect_to: Option<&str>) -> axum::response::Response {
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(behaviour()),
    );
    let cookie = create_test_session(&sessions).await;
    let mut body = "csrf_token=test-csrf&release_intent_id=intent-rel".to_string();
    if let Some(r) = redirect_to {
        body.push_str(&format!("&redirect_to={}", urlencoding::encode(r)));
    }
    build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/orgs/testorg/projects/my-api/plan-stages/plan-prod/{action}"))
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_plan_decision_never_redirects_off_site() {
    for action in ["approve", "reject"] {
        for target in [
            "https://evil.example/phish",
            "//evil.example/phish",
            "/\\evil.example",
            "javascript:alert(1)",
            "releases",
            "/\t/evil.example",
            "/\n/evil.example",
        ] {
            let response = decide_plan(action, Some(target)).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "{action} {target}");
            assert_eq!(
                response.headers()["location"],
                "/orgs/testorg/projects/my-api/releases",
                "{action} {target}"
            );
        }
    }
}

#[tokio::test]
async fn a_plan_decision_follows_a_same_site_path_and_answers_json_without_one() {
    for action in ["approve", "reject"] {
        let response = decide_plan(action, Some(PAGE)).await;
        assert_eq!(response.headers()["location"], PAGE, "{action}");
        let response = decide_plan(action, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{action}");
    }
}
