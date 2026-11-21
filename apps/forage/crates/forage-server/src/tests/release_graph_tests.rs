use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use forage_core::platform::{
    Artifact, ArtifactContext, ArtifactDestination, DeploymentStates, DestinationState,
    PipelineRunStageState, PolicyEvaluation, ReleaseIntentState, ReleaseStepState,
};
use regex::Regex;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const PAGE: &str = "/orgs/testorg/projects/my-api/releases/graph-release";
const FRAGMENT: &str = "/orgs/testorg/projects/my-api/releases/graph-release/graph";
const WAITING: &str =
    "waiting for release brave-otter to apply its plan for prod-eu before planning";

fn artifact() -> Artifact {
    Artifact {
        artifact_id: "graph-artifact".into(),
        slug: "graph-release".into(),
        context: ArtifactContext {
            title: "Graph release".into(),
            description: None,
            web: None,
            pr: None,
        },
        source: None,
        git_ref: None,
        destinations: vec![],
        created_at: "2026-11-21T12:00:00Z".into(),
    }
}

fn stage(id: &str, status: &str, dependencies: &[&str]) -> PipelineRunStageState {
    PipelineRunStageState {
        stage_id: id.into(),
        stage_type: "deploy".into(),
        environment: Some("prod".into()),
        status: status.into(),
        depends_on: dependencies.iter().map(|value| (*value).into()).collect(),
        ..Default::default()
    }
}

fn step(stage_id: &str, name: &str, status: &str) -> ReleaseStepState {
    ReleaseStepState {
        release_id: format!("release-{stage_id}-{name}"),
        stage_id: Some(stage_id.into()),
        destination_name: name.into(),
        environment: "prod".into(),
        status: status.into(),
        queued_at: Some("2026-11-21T12:00:00Z".into()),
        assigned_at: None,
        started_at: None,
        completed_at: None,
        error_message: None,
    }
}

fn current_destination(name: &str, status: &str) -> DestinationState {
    DestinationState {
        destination_id: format!("destination-{name}"),
        destination_name: name.into(),
        environment: "prod".into(),
        release_id: Some(format!("unrelated-new-release-{name}")),
        artifact_id: Some("unrelated-new-artifact".into()),
        status: Some(status.into()),
        error_message: Some("unrelated current deployment error".into()),
        queued_at: None,
        started_at: None,
        completed_at: None,
        queue_position: None,
    }
}

fn behavior(
    stages: Vec<PipelineRunStageState>,
    steps: Vec<ReleaseStepState>,
) -> MockPlatformBehavior {
    MockPlatformBehavior {
        get_artifact_by_slug_result: Some(Ok(artifact())),
        list_artifacts_result: Some(Ok(vec![artifact()])),
        get_release_intent_states_result: Some(Ok(vec![ReleaseIntentState {
            release_intent_id: "graph-intent".into(),
            artifact_id: "graph-artifact".into(),
            project: "my-api".into(),
            created_at: "2026-11-21T12:00:00Z".into(),
            stages,
            steps,
        }])),
        get_destination_states_result: Some(Ok(DeploymentStates::default())),
        evaluate_policies_result: Some(Ok(vec![])),
        ..Default::default()
    }
}

async fn request(behavior: MockPlatformBehavior, uri: &str) -> (String, String) {
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(behavior),
    );
    let cookie = create_test_session(&sessions).await;
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_SECURITY_POLICY],
        crate::security_headers::ENFORCED_POLICY
    );
    let report_only_csp = response.headers()[header::CONTENT_SECURITY_POLICY_REPORT_ONLY]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(report_only_csp, crate::security_headers::REPORT_ONLY_POLICY);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (String::from_utf8(bytes.to_vec()).unwrap(), report_only_csp)
}

fn attr<'a>(markup: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name}=\"");
    let start = markup
        .find(&prefix)
        .unwrap_or_else(|| panic!("missing {name}: {markup}"))
        + prefix.len();
    let rest = &markup[start..];
    &rest[..rest.find('"').unwrap()]
}

fn graph(html: &str) -> &str {
    let section = Regex::new(r#"<section\b[^>]*\bid="release-graph"[^>]*>"#).unwrap();
    let start = section.find(html).expect("release graph section").start();
    let rest = &html[start..];
    let end = rest.find("</section>").expect("closed release graph") + "</section>".len();
    &rest[..end]
}

#[derive(Debug)]
struct Node<'a> {
    id: &'a str,
    state: &'a str,
    href: &'a str,
    markup: &'a str,
}

fn nodes(graph: &str) -> Vec<Node<'_>> {
    Regex::new(r"(?s)<a\b[^>]*>.*?</a>")
        .unwrap()
        .find_iter(graph)
        .filter(|element| element.as_str().contains("data-node-id="))
        .map(|element| {
            let markup = element.as_str();
            Node {
                id: attr(markup, "data-node-id"),
                state: attr(markup, "data-state"),
                href: attr(markup, "href"),
                markup,
            }
        })
        .collect()
}

fn node_text(node: &Node<'_>) -> String {
    Regex::new(r"(?s)<text\b[^>]*>(.*?)</text>")
        .unwrap()
        .captures_iter(node.markup)
        .fold(String::new(), |mut text, capture| {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&capture[1]);
            text
        })
}

fn node_field<'a>(node: &Node<'a>, marker: &str) -> &'a str {
    let pattern = format!(r#"(?s)<text\b[^>]*\b{marker}(?:="[^"]*")?[^>]*>(.*?)</text>"#);
    Regex::new(&pattern)
        .unwrap()
        .captures(node.markup)
        .unwrap_or_else(|| panic!("missing visible {marker}: {node:?}"))
        .get(1)
        .unwrap()
        .as_str()
}

fn stage_node<'a>(nodes: &'a [Node<'a>], id: &str) -> &'a Node<'a> {
    let href = format!("#stage-{id}");
    let title = Regex::new(&format!(r"<title\b[^>]*>{} —", regex::escape(id))).unwrap();
    nodes
        .iter()
        .find(|node| node.href == href && title.is_match(node.markup))
        .unwrap_or_else(|| panic!("missing stage {id}: {nodes:?}"))
}

fn edges(graph: &str) -> Vec<(&str, &str)> {
    Regex::new(r"<[^>]*\bdata-from=[^>]*>")
        .unwrap()
        .find_iter(graph)
        .map(|element| {
            let markup = element.as_str();
            (attr(markup, "data-from"), attr(markup, "data-to"))
        })
        .collect()
}

fn reaches(edges: &[(&str, &str)], from: &str, to: &str) -> bool {
    let mut pending = vec![from];
    let mut visited = HashSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node) {
            continue;
        }
        for &(source, target) in edges {
            if source == node {
                if target == to {
                    return true;
                }
                pending.push(target);
            }
        }
    }
    false
}

#[tokio::test]
async fn each_execution_state_is_visible_on_its_stage_not_just_in_the_legend() {
    for (status, expected) in [
        ("RUNNING", "running"),
        ("PENDING", "next"),
        ("QUEUED", "queued"),
        ("WAITING", "waiting"),
        ("BLOCKED", "blocked"),
        ("FAILED", "failed"),
        ("SUCCEEDED", "done"),
        ("SKIPPED", "skipped"),
        ("CANCELLED", "cancelled"),
        ("SUPERSEDED", "superseded"),
        ("", "unknown"),
        ("UNRECOGNIZED", "unknown"),
    ] {
        let (html, _) = request(
            behavior(vec![stage("execution", status, &[])], vec![]),
            PAGE,
        )
        .await;
        let diagram = graph(&html);
        let rendered = nodes(diagram);
        assert_eq!(
            stage_node(&rendered, "execution").state,
            expected,
            "{status}: {diagram}"
        );
        let terminal = matches!(
            expected,
            "failed" | "done" | "skipped" | "cancelled" | "superseded"
        );
        assert_eq!(
            attr(diagram, "data-live"),
            if terminal { "false" } else { "true" },
            "{status}: {diagram}"
        );
    }
}

#[tokio::test]
async fn prerequisites_distinguish_ready_work_from_work_waiting_or_blocked() {
    let stages = vec![
        stage("complete", "SUCCEEDED", &[]),
        stage("active", "RUNNING", &[]),
        stage("broken", "FAILED", &[]),
        stage("ready", "PENDING", &["complete"]),
        stage("not-ready", "PENDING", &["active"]),
        stage("blocked", "PENDING", &["broken"]),
    ];
    let (html, _) = request(behavior(stages, vec![]), PAGE).await;
    let diagram = graph(&html);
    let rendered = nodes(diagram);
    assert_eq!(stage_node(&rendered, "ready").state, "next");
    assert_ne!(stage_node(&rendered, "not-ready").state, "next");
    assert_eq!(stage_node(&rendered, "blocked").state, "blocked");
    assert_eq!(attr(diagram, "data-live"), "true");
}

#[tokio::test]
async fn a_plan_held_by_another_release_is_waiting_and_keeps_its_reason() {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        error_message: Some(WAITING.into()),
        ..stage("held-plan", "RUNNING", &[])
    };
    let (html, _) = request(behavior(vec![plan], vec![]), PAGE).await;
    let rendered = nodes(graph(&html));
    let plan = stage_node(&rendered, "held-plan");
    assert_eq!(plan.state, "waiting");
    assert!(plan.markup.contains(WAITING), "{plan:?}");
    assert!(
        node_field(plan, "data-node-preview").contains("waiting for release brave-otter"),
        "the lock reason is visible without hover: {plan:?}"
    );
    assert_eq!(attr(graph(&html), "data-live"), "true");
}

#[tokio::test]
async fn a_completed_destination_does_not_erase_its_stages_plan_lock() {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        error_message: Some(WAITING.into()),
        ..stage("held-plan", "RUNNING", &[])
    };
    let (html, _) = request(
        behavior(
            vec![plan, stage("deploy", "PENDING", &["held-plan"])],
            vec![step("held-plan", "prod-eu", "SUCCEEDED")],
        ),
        PAGE,
    )
    .await;
    let diagram = graph(&html);
    let rendered = nodes(diagram);
    let plan = stage_node(&rendered, "held-plan");
    let destination = rendered
        .iter()
        .find(|node| {
            node.href == "#stage-held-plan" && attr(node.markup, "data-node-label") == "prod-eu"
        })
        .expect("completed destination plan");
    assert_eq!(plan.state, "waiting");
    assert_eq!(destination.state, "done");
    assert_eq!(
        stage_node(&rendered, "deploy").state,
        "waiting",
        "the aggregate plan still holds the dependent deploy: {diagram}"
    );
    assert!(reaches(
        &edges(diagram),
        destination.id,
        stage_node(&rendered, "deploy").id
    ));
    assert_eq!(attr(diagram, "data-live"), "true");
}

#[tokio::test]
async fn stage_cards_count_only_their_destination_runs_and_show_environment_and_duration() {
    let mut plan = stage("plan", "RUNNING", &[]);
    plan.stage_type = "plan".into();
    plan.environment = Some("production".into());
    plan.duration_seconds = Some(17);
    let mut deploy = stage("deploy", "RUNNING", &["plan"]);
    deploy.environment = Some("production".into());
    deploy.duration_seconds = Some(23);
    let mut executions = vec![
        step("plan", "eu", "SUCCEEDED"),
        step("plan", "us", "SUCCEEDED"),
        step("plan", "ap", "RUNNING"),
        step("deploy", "eu", "SUCCEEDED"),
        step("deploy", "us", "QUEUED"),
    ];
    for execution in &mut executions {
        execution.environment = "production".into();
        execution.started_at = Some("2026-11-21T12:00:00Z".into());
        execution.completed_at = Some("2026-11-21T12:00:09Z".into());
    }
    let (html, _) = request(behavior(vec![plan, deploy], executions), PAGE).await;
    let rendered = nodes(graph(&html));
    for (id, complete, total, duration) in [("plan", 2, 3, "17s"), ("deploy", 1, 2, "23s")] {
        let node = stage_node(&rendered, id);
        let summary = node_field(node, "data-node-summary");
        assert!(
            Regex::new(&format!(r"\b{complete}\s*/\s*{total}\b"))
                .unwrap()
                .is_match(summary),
            "{id} counts its own completed and total destination runs: {summary}"
        );
        assert!(summary.contains(duration), "{node:?}");
        assert_eq!(node_field(node, "data-node-environment"), "production");
    }
    let destination = rendered
        .iter()
        .find(|node| {
            node.href == "#stage-plan"
                && node_text(node).split_whitespace().any(|text| text == "eu")
        })
        .expect("plan destination");
    assert!(node_text(destination).contains("9s"), "{destination:?}");
    assert_eq!(
        node_field(destination, "data-node-environment"),
        "production"
    );
}

#[tokio::test]
async fn destination_plans_share_one_escaped_output_template_for_the_inspector() {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        ..stage("plan", "SUCCEEDED", &[])
    };
    let mut fixture = behavior(
        vec![plan],
        vec![
            step("plan", "prod-eu", "SUCCEEDED"),
            step("plan", "prod-us", "SUCCEEDED"),
            step("plan", "prod-ap", "SUCCEEDED"),
        ],
    );
    fixture.get_plan_output_result = Some(Ok(forage_core::platform::PlanOutput {
        plan_output: "+ ledger_table\n~ api replicas: 2 < 3 & ready".into(),
        status: "SUCCEEDED".into(),
        outputs: vec![],
    }));
    let (html, _) = request(fixture, PAGE).await;
    let diagram = graph(&html);
    let templates: Vec<_> =
        Regex::new(r"(?s)<template\b[^>]*\bdata-graph-plan=[^>]*>.*?</template>")
            .unwrap()
            .find_iter(diagram)
            .map(|element| element.as_str())
            .collect();
    assert_eq!(templates.len(), 1, "one payload per plan, not per node");
    let template = templates[0];
    assert!(template.contains("<pre"), "{template}");
    assert!(template.contains("+ ledger_table"), "{template}");
    let escaped_line = "~ api replicas: 2 &lt; 3 &amp; ready";
    assert!(template.contains(escaped_line), "{template}");
    assert_eq!(diagram.matches(escaped_line).count(), 1);
    assert!(!diagram.contains("2 < 3 & ready"));
    let rendered = nodes(diagram);
    let plan_index = attr(template, "data-graph-plan");
    assert_eq!(rendered.len(), 4);
    for node in &rendered {
        assert_eq!(attr(node.markup, "data-plan-index"), plan_index);
        assert!(!node.markup.contains(escaped_line), "{node:?}");
        assert!(
            node_field(node, "data-node-preview").contains("ledger_table"),
            "plan context appears directly in the card: {node:?}"
        );
    }
}

#[tokio::test]
async fn parallel_destinations_keep_their_own_stage_steps_and_dependency_paths() {
    let stages = vec![
        PipelineRunStageState {
            stage_type: "plan".into(),
            ..stage("plan", "SUCCEEDED", &[])
        },
        stage("deploy", "RUNNING", &["plan"]),
        stage("independent", "RUNNING", &[]),
    ];
    let steps = vec![
        step("plan", "prod-eu", "SUCCEEDED"),
        step("plan", "prod-us", "SUCCEEDED"),
        step("deploy", "prod-eu", "RUNNING"),
        step("deploy", "prod-us", "QUEUED"),
    ];
    let mut fixture = behavior(stages, steps);
    fixture.get_destination_states_result = Some(Ok(DeploymentStates {
        destinations: vec![
            current_destination("prod-eu", "FAILED"),
            current_destination("prod-us", "CANCELLED"),
        ],
    }));
    let (html, _) = request(fixture, PAGE).await;
    let diagram = graph(&html);
    let rendered = nodes(diagram);
    let paths = edges(diagram);
    let plan = stage_node(&rendered, "plan");
    let deploy = stage_node(&rendered, "deploy");
    let independent = stage_node(&rendered, "independent");
    assert_eq!(
        plan.state, "done",
        "historical stage is not a current destination projection"
    );
    assert_eq!(deploy.state, "running");
    assert!(reaches(&paths, plan.id, deploy.id), "{diagram}");
    assert!(
        !reaches(&paths, plan.id, independent.id),
        "independent branch: {diagram}"
    );
    assert!(
        !reaches(&paths, independent.id, deploy.id),
        "independent branch: {diagram}"
    );
    for (name, active_state) in [("prod-eu", "running"), ("prod-us", "queued")] {
        let children: Vec<_> = rendered
            .iter()
            .filter(|node| node.markup.contains(name))
            .collect();
        assert_eq!(
            children.len(),
            2,
            "one plan and one deploy execution for {name}: {diagram}"
        );
        let planned = children
            .iter()
            .find(|node| node.state == "done")
            .expect("completed plan step");
        let deploying = children
            .iter()
            .find(|node| node.state == active_state)
            .expect("independent deploy step state");
        assert_eq!(planned.href, "#stage-plan", "plan output remains reachable");
        assert!(
            planned.markup.contains("plan"),
            "plan stage context: {planned:?}"
        );
        assert_eq!(
            deploying.href, "#stage-deploy",
            "historic execution links to its own stage"
        );
        assert!(
            deploying.markup.contains("deploy"),
            "deploy stage context: {deploying:?}"
        );
        assert!(
            reaches(&paths, plan.id, planned.id),
            "plan owns its step: {diagram}"
        );
        assert!(
            reaches(&paths, planned.id, deploy.id),
            "plan step gates deploy: {diagram}"
        );
        assert!(
            reaches(&paths, deploy.id, deploying.id),
            "deploy owns its step: {diagram}"
        );
        assert!(
            !reaches(&paths, deploying.id, deploy.id),
            "no backwards dependency: {diagram}"
        );
    }
    assert!(
        !diagram.contains("unrelated current deployment error"),
        "{diagram}"
    );
}

#[tokio::test]
async fn hostile_labels_and_failure_reasons_remain_text_and_graph_ids_remain_internal() {
    let hostile = "prod-<script>alert(1)</script>&\"";
    let reason = "failed <img src=x onerror=alert(1)> & \"quoted\"";
    let mut execution = step("deploy", hostile, "FAILED");
    execution.error_message = Some(reason.into());
    let (html, _) = request(
        behavior(vec![stage("deploy", "FAILED", &[])], vec![execution]),
        PAGE,
    )
    .await;
    let diagram = graph(&html);
    assert!(!diagram.contains(hostile));
    assert!(!diagram.contains(reason));
    assert!(
        diagram.contains("prod-&lt;script&gt;alert(1)&lt;/script&gt;&amp;&quot;"),
        "{diagram}"
    );
    assert!(
        diagram.contains("failed &lt;img src=x onerror=alert(1)&gt; &amp; &quot;quoted&quot;"),
        "{diagram}"
    );
    let safe_id = Regex::new(r"^graph-node-[0-9]+$").unwrap();
    let rendered = nodes(diagram);
    assert!(!rendered.is_empty());
    let failed = rendered
        .iter()
        .find(|node| {
            node.href == "#stage-deploy"
                && node.state == "failed"
                && node.markup.contains("prod-&lt;script&gt;")
        })
        .expect("failed destination node");
    assert!(
        node_field(failed, "data-node-preview")
            .contains("failed &lt;img src=x onerror=alert(1)&gt; &amp; &quot;quoted&quot;"),
        "the failure reason is visible in the card: {failed:?}"
    );
    let mut ids = HashSet::new();
    for node in &rendered {
        assert!(safe_id.is_match(node.id), "{node:?}");
        assert!(ids.insert(node.id), "duplicate node id: {node:?}");
    }
    for (from, to) in edges(diagram) {
        assert!(
            ids.contains(from) && ids.contains(to),
            "dangling edge {from} -> {to}"
        );
    }
}

#[tokio::test]
async fn the_above_fold_workspace_keeps_existing_content_and_reuses_existing_actions() {
    let mut fixture = behavior(
        vec![stage("deploy", "RUNNING", &[])],
        vec![step("deploy", "prod-eu", "RUNNING")],
    );
    fixture.get_destination_states_result = Some(Ok(DeploymentStates {
        destinations: vec![DestinationState {
            artifact_id: Some("graph-artifact".into()),
            ..current_destination("prod-eu", "RUNNING")
        }],
    }));
    fixture.get_artifact_spec_result = Some(Ok("project: \"my-api\"".into()));
    let (html, _) = request(fixture, PAGE).await;
    let diagram = graph(&html);
    let graph_start = html.find(diagram).unwrap();
    for marker in [
        "id=\"stage-deploy\"",
        "id=\"dest-prod-eu\"",
        "<release-logs",
        "<spec-viewer",
    ] {
        assert!(
            html.find(marker)
                .unwrap_or_else(|| panic!("missing {marker}"))
                > graph_start,
            "{marker} must remain after the above-fold node viewer"
        );
    }
    assert!(diagram.contains("<svg"), "server-rendered SVG");
    assert!(
        !diagram.contains("<form"),
        "the inspector reuses existing actions"
    );
    for marker in [
        "data-graph-inspector",
        "data-graph-tab=\"details\"",
        "data-graph-tab=\"plan\"",
        "data-graph-tab=\"logs\"",
        "data-graph-action=\"fit\"",
        "data-graph-action=\"active\"",
        "data-graph-action=\"fullscreen\"",
        "data-graph-viewport",
    ] {
        assert!(
            diagram.contains(marker),
            "missing workspace control {marker}"
        );
    }
    let rendered = nodes(diagram);
    assert!(!rendered.is_empty());
    for node in rendered {
        assert!(
            node.href.starts_with('#') && node.href.len() > 1,
            "keyboard-accessible node with an existing-section fallback: {node:?}"
        );
        let target = format!("id=\"{}\"", &node.href[1..]);
        assert!(
            html.contains(&target),
            "missing inspector action source {target}"
        );
        assert!(
            !node.markup.contains("tabindex=\"-1\""),
            "node removed from keyboard order"
        );
    }
}

#[tokio::test]
async fn approval_gates_and_policy_reasons_keep_the_graph_live_without_duplicate_actions() {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        approval_status: Some("AWAITING_APPROVAL".into()),
        ..stage("approval", "RUNNING", &[])
    };
    let mut fixture = behavior(vec![plan], vec![]);
    fixture.evaluate_policies_result = Some(Ok(vec![PolicyEvaluation {
        policy_name: "two-person-rule".into(),
        policy_type: "approval".into(),
        passed: false,
        reason: "0 of 2 approvals <required>".into(),
        approval_state: None,
    }]));
    let (html, _) = request(fixture, PAGE).await;
    let diagram = graph(&html);
    assert_eq!(attr(diagram, "data-live"), "true");
    assert!(
        nodes(diagram)
            .iter()
            .any(|node| node.state == "waiting" || node.state == "blocked"),
        "{diagram}"
    );
    assert!(diagram.contains("two-person-rule"), "{diagram}");
    assert!(
        diagram.contains("0 of 2 approvals &lt;required&gt;"),
        "{diagram}"
    );
    let rendered = nodes(diagram);
    let policy = rendered
        .iter()
        .find(|node| node.markup.contains("two-person-rule"))
        .expect("policy node");
    assert!(
        node_field(policy, "data-node-preview").contains("0 of 2 approvals &lt;required&gt;"),
        "the policy reason appears directly in its card: {policy:?}"
    );
    assert!(diagram.contains("data-graph-inspector"));
    let forms: Vec<_> = Regex::new(r"(?s)<form\b[^>]*>.*?</form>")
        .unwrap()
        .find_iter(&html)
        .map(|element| element.as_str())
        .collect();
    for action in ["approve", "reject"] {
        let action_path = format!("/plan-stages/approval/{action}");
        let forms: Vec<_> = forms
            .iter()
            .filter(|form| attr(form, "action").ends_with(&action_path))
            .collect();
        assert_eq!(
            forms.len(),
            1,
            "one existing authenticated plan {action} form"
        );
        assert!(forms[0].contains("name=\"csrf_token\""), "{action}");
    }
    assert!(!diagram.contains("<form"));
}

#[tokio::test]
async fn configured_destinations_are_visible_before_any_intent_has_executed() {
    let mut art = artifact();
    art.destinations.push(ArtifactDestination {
        name: "prod-configured".into(),
        environment: "prod".into(),
        type_organisation: None,
        type_name: None,
        type_version: None,
        status: None,
    });
    let mut fixture = behavior(vec![], vec![]);
    fixture.get_artifact_by_slug_result = Some(Ok(art));
    fixture.get_release_intent_states_result = Some(Ok(vec![]));
    let (html, _) = request(fixture, PAGE).await;
    let rendered = nodes(graph(&html));
    let destination = rendered
        .iter()
        .find(|node| node.markup.contains("prod-configured"))
        .expect("configured destination node");
    assert_ne!(destination.state, "done", "never-run work is not complete");
    assert!(destination.markup.contains("prod-configured"));
    let target = format!(
        "id=\"{}\"",
        destination.href.strip_prefix('#').expect("section anchor")
    );
    assert!(
        html.contains(&target),
        "configured destination anchor exists"
    );
}

#[tokio::test]
async fn the_graph_adds_no_inline_code_or_styles_under_the_page_csp() {
    let (html, report_only_csp) = request(
        behavior(vec![stage("deploy", "RUNNING", &[])], vec![]),
        PAGE,
    )
    .await;
    assert!(report_only_csp.contains("script-src 'self';"));
    assert!(report_only_csp.contains("style-src 'self';"));
    assert!(!report_only_csp.contains("'unsafe-inline'"));
    let inline = Regex::new(r#"(?i)<style\b|\sstyle\s*=|\son[a-z]+\s*="#).unwrap();
    assert!(!inline.is_match(&html), "inline style or event handler");
    let scripts = Regex::new(r"(?is)<script\b([^>]*)>(.*?)</script>").unwrap();
    for script in scripts.captures_iter(&html) {
        assert!(
            script[1].contains("src=\"/static/"),
            "scripts are self-hosted: {}",
            &script[0]
        );
        assert!(
            script[2].trim().is_empty(),
            "no inline script: {}",
            &script[0]
        );
    }
}

#[tokio::test]
async fn a_failed_prerequisite_blocks_descendants_even_when_stages_arrive_out_of_order() {
    let (html, _) = request(
        behavior(
            vec![
                stage("last", "PENDING", &["middle"]),
                stage("middle", "PENDING", &["first"]),
                stage("first", "FAILED", &[]),
            ],
            vec![],
        ),
        PAGE,
    )
    .await;
    let rendered = nodes(graph(&html));
    assert_eq!(stage_node(&rendered, "middle").state, "blocked");
    assert_eq!(stage_node(&rendered, "last").state, "blocked");
}

#[tokio::test]
async fn missing_and_cyclic_dependencies_are_explained_instead_of_advertised_as_next() {
    let (html, _) = request(
        behavior(
            vec![
                stage("missing", "PENDING", &["not-reported"]),
                stage("cycle-a", "PENDING", &["cycle-b"]),
                stage("cycle-b", "PENDING", &["cycle-a"]),
            ],
            vec![],
        ),
        PAGE,
    )
    .await;
    let rendered = nodes(graph(&html));
    for id in ["missing", "cycle-a", "cycle-b"] {
        assert_eq!(stage_node(&rendered, id).state, "blocked");
    }
    assert!(graph(&html).contains("Dependency not-reported was not reported"));
    assert!(graph(&html).contains("Cyclic pipeline dependency"));
}

#[tokio::test]
async fn a_passed_policy_does_not_make_deploy_next_before_plan_approval() {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        approval_status: Some("AWAITING_APPROVAL".into()),
        ..stage("plan", "RUNNING", &[])
    };
    let mut fixture = behavior(vec![plan, stage("deploy", "PENDING", &["plan"])], vec![]);
    fixture.evaluate_policies_result = Some(Ok(vec![PolicyEvaluation {
        policy_name: "soak".into(),
        policy_type: "soak_time".into(),
        passed: true,
        reason: "soak completed".into(),
        approval_state: None,
    }]));
    let (html, _) = request(fixture, PAGE).await;
    let rendered = nodes(graph(&html));
    assert_eq!(stage_node(&rendered, "deploy").state, "waiting");
    assert!(
        stage_node(&rendered, "deploy")
            .markup
            .contains("Plan approval")
    );
}

#[tokio::test]
async fn the_compact_workspace_retains_completed_active_and_future_nodes() {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        approval_status: Some("AWAITING_APPROVAL".into()),
        ..stage("plan", "RUNNING", &["complete"])
    };
    let (html, _) = request(
        behavior(
            vec![
                stage("complete", "SUCCEEDED", &[]),
                plan,
                stage("future", "PENDING", &["plan"]),
            ],
            vec![],
        ),
        PAGE,
    )
    .await;
    let diagram = graph(&html);
    let rendered = nodes(diagram);
    assert_eq!(stage_node(&rendered, "complete").state, "done");
    assert_eq!(stage_node(&rendered, "future").state, "waiting");
    assert!(rendered.iter().any(
        |node| attr(node.markup, "data-node-label") == "Plan approval" && node.state == "waiting"
    ));
    assert!(html.find(diagram).unwrap() < html.find("id=\"pipeline\"").unwrap());
}

#[tokio::test]
async fn terminal_releases_keep_the_full_graph_instead_of_disappearing_with_the_queue() {
    let (html, _) = request(
        behavior(
            vec![stage("deploy", "SUCCEEDED", &[])],
            vec![step("deploy", "prod-eu", "SUCCEEDED")],
        ),
        PAGE,
    )
    .await;
    let diagram = graph(&html);
    let rendered = nodes(diagram);
    assert_eq!(attr(diagram, "data-live"), "false");
    assert_eq!(stage_node(&rendered, "deploy").state, "done");
    assert!(
        rendered
            .iter()
            .any(|node| attr(node.markup, "data-node-label") == "prod-eu" && node.state == "done")
    );
}

#[tokio::test]
async fn timeline_stage_dependencies_preserve_branches_for_the_overview_flow() {
    let mut fixture = behavior(
        vec![
            PipelineRunStageState {
                stage_type: "plan".into(),
                ..stage("first", "SUCCEEDED", &[])
            },
            stage("dependent", "PENDING", &["first"]),
            stage("independent", "RUNNING", &[]),
        ],
        vec![step("first", "prod-eu", "SUCCEEDED")],
    );
    fixture.list_artifacts_result = Some(Ok(vec![artifact()]));
    let (body, _) = request(fixture, "/api/orgs/testorg/projects/my-api/timeline").await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let release = body["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|item| {
            item.get("release")
                .or_else(|| item.get("releases").and_then(|releases| releases.get(0)))
        })
        .expect("release in timeline");
    let stages = release["pipeline_stages"].as_array().unwrap();
    let dependent = stages
        .iter()
        .find(|stage| stage["id"] == "dependent")
        .unwrap();
    let independent = stages
        .iter()
        .find(|stage| stage["id"] == "independent")
        .unwrap();
    assert_eq!(dependent["depends_on"], serde_json::json!(["first"]));
    assert_eq!(independent["depends_on"], serde_json::json!([]));
    assert_eq!(
        release["destinations"][0]["stage_id"], "first",
        "completed plan provenance must not turn into a deployed destination"
    );
}

fn timeline_releases(body: &serde_json::Value) -> Vec<&serde_json::Value> {
    body["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| {
            if let Some(release) = item.get("release") {
                vec![release]
            } else {
                item["releases"]
                    .as_array()
                    .map(|releases| releases.iter().collect())
                    .unwrap_or_default()
            }
        })
        .collect()
}

#[tokio::test]
async fn the_lazy_fragment_uses_the_same_full_node_projection_as_release_details() {
    fn fixture() -> MockPlatformBehavior {
        behavior(
            vec![
                stage("complete", "SUCCEEDED", &[]),
                PipelineRunStageState {
                    stage_type: "plan".into(),
                    approval_status: Some("AWAITING_APPROVAL".into()),
                    ..stage("plan", "RUNNING", &["complete"])
                },
                stage("future", "PENDING", &["plan"]),
            ],
            vec![step("plan", "prod-eu", "SUCCEEDED")],
        )
    }
    let (detail, _) = request(fixture(), PAGE).await;
    let (fragment, _) = request(fixture(), FRAGMENT).await;
    let signature = |html: &str| {
        let mut values: Vec<_> = nodes(html)
            .iter()
            .map(|node| {
                (
                    attr(node.markup, "data-node-label").to_owned(),
                    node.state.to_owned(),
                    attr(node.markup, "data-node-kind").to_owned(),
                )
            })
            .collect();
        values.sort();
        values
    };
    assert_eq!(signature(&fragment), signature(graph(&detail)));
    assert!(
        signature(&fragment)
            .iter()
            .any(|(label, state, _)| label == "complete" && state == "done")
    );
    assert!(
        signature(&fragment)
            .iter()
            .any(|(label, state, _)| label == "future" && state == "waiting")
    );
    assert!(fragment.contains("data-poll=\"false\""));
    assert!(
        !fragment.contains("<form"),
        "approval forms stay on the release page"
    );
    assert!(fragment.contains("data-release-intent-id=\"graph-intent\""));
    assert!(
        fragment
            .contains("data-plan-output-url=\"/api/orgs/testorg/projects/my-api/plan-stages/\"")
    );
    assert!(
        fragment.contains("data-plan-stage=\"plan\""),
        "empty output retains lazy plan metadata"
    );
    let plan_nodes: Vec<_> = nodes(&fragment)
        .into_iter()
        .filter(|node| matches!(attr(node.markup, "data-node-kind"), "plan" | "approval"))
        .collect();
    assert_eq!(
        plan_nodes.len(),
        3,
        "stage, destination and approval share one plan record"
    );
    assert!(
        plan_nodes
            .iter()
            .all(|node| attr(node.markup, "data-plan-index") == "0")
    );
}

fn cost_fixture(calls: Arc<Mutex<Vec<String>>>) -> MockPlatformBehavior {
    let mut value = behavior(
        vec![
            PipelineRunStageState {
                stage_type: "plan".into(),
                ..stage("plan", "RUNNING", &[])
            },
            stage("deploy", "PENDING", &["plan"]),
        ],
        vec![],
    );
    let mut artifacts = vec![];
    let mut intents = vec![];
    for index in 0..12 {
        let mut release = artifact();
        release.artifact_id = format!("artifact-{index}");
        release.slug = format!("release-{index}");
        let mut intent = value
            .get_release_intent_states_result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()[0]
            .clone();
        intent.artifact_id = release.artifact_id.clone();
        intent.release_intent_id = format!("intent-{index}");
        artifacts.push(release);
        intents.push(intent);
    }
    value.list_artifacts_result = Some(Ok(artifacts));
    value.get_release_intent_states_result = Some(Ok(intents));
    value.get_plan_output_result = Some(Ok(forage_core::platform::PlanOutput {
        plan_output: "HUGE_PRIVATE_PLAN_OUTPUT".repeat(100_000),
        status: "SUCCEEDED".into(),
        outputs: vec![],
    }));
    value.graph_rpc_calls = Some(calls);
    value
}

#[tokio::test]
async fn default_timelines_skip_graphs_and_opted_in_environment_graphs_add_zero_rpcs() {
    for endpoint in [
        "/api/orgs/testorg/projects/my-api/timeline",
        "/api/orgs/testorg/timeline",
    ] {
        let default_calls = Arc::new(Mutex::new(vec![]));
        let (default, _) = request(cost_fixture(default_calls.clone()), endpoint).await;
        let default: serde_json::Value = serde_json::from_str(&default).unwrap();
        assert!(default.get("graph_html").is_none());
        assert!(
            timeline_releases(&default)
                .iter()
                .all(|release| release.get("graph_html").is_none())
        );
        let graph_calls = Arc::new(Mutex::new(vec![]));
        let (with_graph, _) = request(
            cost_fixture(graph_calls.clone()),
            &format!("{endpoint}?include_graph=true"),
        )
        .await;
        let with_graph: serde_json::Value = serde_json::from_str(&with_graph).unwrap();
        let fragment = with_graph["graph_html"]
            .as_str()
            .expect("one top-level environment workspace");
        assert!(fragment.contains("id=\"release-environments\""));
        assert!(!fragment.contains("data-graph-tab=\"plan\""));
        assert!(!fragment.contains("data-graph-tab=\"logs\""));
        assert!(!fragment.contains("data-graph-plan="));
        assert!(!fragment.contains("HUGE_PRIVATE_PLAN_OUTPUT"));
        assert!(
            timeline_releases(&with_graph)
                .iter()
                .all(|release| release.get("graph_html").is_none())
        );
        let mut expected = default_calls.lock().unwrap().clone();
        let mut actual = graph_calls.lock().unwrap().clone();
        expected.sort();
        actual.sort();
        assert_eq!(
            actual, expected,
            "global projection adds zero upstream RPCs"
        );
        assert!(
            !actual
                .iter()
                .any(|call| call == "get_plan_output" || call.starts_with("evaluate_policies:"))
        );
    }
}

#[tokio::test]
async fn lazy_graph_evaluates_only_the_latest_requested_intent_and_never_fetches_or_embeds_plan_output()
 {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut fixture = behavior(
        vec![
            PipelineRunStageState {
                stage_type: "plan".into(),
                ..stage("plan", "RUNNING", &[])
            },
            stage("deploy", "PENDING", &["plan"]),
        ],
        vec![],
    );
    let old = fixture
        .get_release_intent_states_result
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()[0]
        .clone();
    let mut latest = old.clone();
    latest.release_intent_id = "latest-intent".into();
    latest.created_at = "2026-11-21T13:00:00Z".into();
    let mut unrelated = old.clone();
    unrelated.artifact_id = "other-artifact".into();
    unrelated.release_intent_id = "other-intent".into();
    fixture.get_release_intent_states_result = Some(Ok(vec![latest, unrelated, old]));
    fixture.get_plan_output_result = Some(Ok(forage_core::platform::PlanOutput {
        plan_output: "HUGE_PRIVATE_PLAN_OUTPUT".repeat(100_000),
        status: "SUCCEEDED".into(),
        outputs: vec![],
    }));
    fixture.graph_rpc_calls = Some(calls.clone());
    let (fragment, _) = request(fixture, FRAGMENT).await;
    assert!(!fragment.contains("HUGE_PRIVATE_PLAN_OUTPUT"));
    assert!(
        fragment.len() < 64_000,
        "plan size cannot inflate a lazy pipeline fragment"
    );
    assert!(fragment.contains("data-release-intent-id=\"latest-intent\""));
    assert!(fragment.contains("data-plan-stage=\"plan\""));
    let mut actual = calls.lock().unwrap().clone();
    actual.sort();
    assert_eq!(
        actual,
        vec![
            "evaluate_policies:my-api:prod:latest-intent",
            "get_release_intent_states",
            "list_artifacts",
        ]
    );
}

#[tokio::test]
async fn one_global_workspace_shows_current_destinations_across_projects_not_latest_artifact_runs()
{
    let first = artifact();
    let mut second = artifact();
    second.artifact_id = "worker-artifact".into();
    second.slug = "worker-release".into();
    second.context.title = "Worker release".into();
    let mut historical = artifact();
    historical.artifact_id = "newer-not-deployed".into();
    historical.slug = "newer-release".into();
    historical.context.title = "Newer but not deployed".into();
    historical.created_at = "2026-11-21T14:00:00Z".into();
    let mut fixture = behavior(vec![stage("historical-failure", "FAILED", &[])], vec![]);
    fixture.list_projects_result = Some(Ok(vec!["my-api".into(), "worker".into()]));
    fixture.list_artifacts_by_project = Some(std::collections::HashMap::from([
        ("my-api".into(), vec![historical, first]),
        ("worker".into(), vec![second]),
    ]));
    fixture.get_destination_states_result = Some(Ok(DeploymentStates {
        destinations: vec![
            DestinationState {
                artifact_id: Some("graph-artifact".into()),
                error_message: None,
                ..current_destination("api-eu", "RUNNING")
            },
            DestinationState {
                artifact_id: Some("worker-artifact".into()),
                error_message: Some("worker cannot start".into()),
                ..current_destination("worker-eu", "FAILED")
            },
        ],
    }));
    let (body, _) = request(fixture, "/api/orgs/testorg/timeline?include_graph=true").await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let fragment = body["graph_html"].as_str().unwrap();
    assert_eq!(fragment.matches("data-release-graph").count(), 1);
    let nodes = nodes(fragment);
    assert_eq!(
        nodes.len(),
        3,
        "one environment and both current destinations"
    );
    assert!(nodes.iter().all(|node| matches!(
        attr(node.markup, "data-node-kind"),
        "environment" | "destination"
    )));
    let api = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-label") == "api-eu")
        .unwrap();
    assert_eq!(api.state, "running");
    assert_eq!(api.href, PAGE);
    assert!(attr(api.markup, "data-node-context").contains("my-api"));
    assert!(attr(api.markup, "data-node-context").contains("Graph release"));
    assert!(!fragment.contains("Newer but not deployed"));
    let worker = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-label") == "worker-eu")
        .unwrap();
    assert_eq!(worker.state, "failed");
    assert!(worker.href.ends_with("/worker/releases/worker-release"));
    let environment = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-kind") == "environment")
        .unwrap();
    assert_eq!(environment.state, "failed");
    let edges = edges(fragment);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|(from, _)| *from == environment.id));
}

#[tokio::test]
async fn successful_plan_operations_are_not_presented_as_deployments_and_missing_provenance_is_explicit()
 {
    let plan = PipelineRunStageState {
        stage_type: "plan".into(),
        ..stage("plan", "SUCCEEDED", &[])
    };
    let execution = step("plan", "prod-eu", "SUCCEEDED");
    let mut fixture = behavior(vec![plan], vec![execution.clone()]);
    fixture.get_destination_states_result = Some(Ok(DeploymentStates {
        destinations: vec![
            DestinationState {
                artifact_id: Some("graph-artifact".into()),
                release_id: Some(execution.release_id),
                error_message: None,
                ..current_destination("prod-eu", "SUCCEEDED")
            },
            DestinationState {
                error_message: None,
                ..current_destination("unknown-owner", "SUCCEEDED")
            },
            DestinationState {
                error_message: None,
                ..current_destination("incoming", "PENDING")
            },
        ],
    }));
    let (body, _) = request(
        fixture,
        "/api/orgs/testorg/projects/my-api/timeline?include_graph=true",
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let nodes = nodes(body["graph_html"].as_str().unwrap());
    let plan = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-label") == "prod-eu")
        .unwrap();
    assert_eq!(plan.state, "done");
    assert!(attr(plan.markup, "data-node-summary").contains("Plan complete · not deployed"));
    let unknown = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-label") == "unknown-owner")
        .unwrap();
    assert_eq!(
        unknown.state, "done",
        "operation success is known even when its type is missing"
    );
    assert!(attr(unknown.markup, "data-node-context").contains("Release provenance unavailable"));
    assert_eq!(
        attr(unknown.markup, "data-node-summary"),
        "Operation provenance unavailable"
    );
    assert!(
        unknown
            .href
            .contains("/destinations/detail?name=unknown-owner")
    );
    let incoming = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-label") == "incoming")
        .unwrap();
    assert_eq!(
        incoming.state, "next",
        "environment containment is not an admission dependency"
    );
}

#[tokio::test]
async fn graph_fragments_escape_hostile_titles_and_reasons_without_loading_hostile_plan_output() {
    let title = "<img src=x onerror=alert(1)>";
    let failure = "</svg><script>alert(1)</script>";
    for endpoint in [
        FRAGMENT,
        "/api/orgs/testorg/projects/my-api/timeline?include_graph=true",
    ] {
        let mut release = artifact();
        release.context.title = title.into();
        let mut execution = step("deploy", title, "FAILED");
        execution.error_message = Some(failure.into());
        let mut fixture = behavior(
            vec![
                PipelineRunStageState {
                    stage_type: "plan".into(),
                    ..stage("plan", "SUCCEEDED", &[])
                },
                stage("deploy", "FAILED", &["plan"]),
            ],
            vec![execution],
        );
        fixture.list_artifacts_result = Some(Ok(vec![release]));
        fixture.get_destination_states_result = Some(Ok(DeploymentStates {
            destinations: vec![DestinationState {
                artifact_id: Some("graph-artifact".into()),
                error_message: Some(failure.into()),
                ..current_destination(title, "FAILED")
            }],
        }));
        fixture.get_plan_output_result = Some(Ok(forage_core::platform::PlanOutput {
            plan_output: "PRIVATE_HOSTILE_PLAN</script>".into(),
            status: "SUCCEEDED".into(),
            outputs: vec![],
        }));
        fixture.evaluate_policies_result = Some(Ok(vec![PolicyEvaluation {
            policy_name: title.into(),
            policy_type: "soak_time".into(),
            passed: false,
            reason: failure.into(),
            approval_state: None,
        }]));
        let (body, _) = request(fixture, endpoint).await;
        let fragment = if endpoint == FRAGMENT {
            body
        } else {
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(timeline_releases(&json)[0]["title"], title);
            json["graph_html"].as_str().unwrap().to_owned()
        };
        assert!(fragment.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(fragment.contains("&lt;/svg&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!fragment.contains(title) && !fragment.contains(failure));
        assert!(!fragment.contains("PRIVATE_HOSTILE_PLAN"));
        assert!(
            !Regex::new(r"(?i)<(?:img|script)\b")
                .unwrap()
                .is_match(&fragment)
        );
        assert!(nodes(&fragment).iter().any(|node| node.state == "failed"
            && attr(node.markup, "data-node-label").contains("&lt;img")));
    }
}

#[tokio::test]
async fn lazy_graphs_require_a_session_and_org_membership_before_any_platform_rpc() {
    for (uri, signed_in) in [
        (FRAGMENT, false),
        (
            "/orgs/otherorg/projects/my-api/releases/graph-release/graph",
            true,
        ),
    ] {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut fixture = behavior(vec![], vec![]);
        fixture.graph_rpc_calls = Some(calls.clone());
        let (state, sessions) = test_state_with(
            MockForestClient::new(),
            MockPlatformClient::with_behavior(fixture),
        );
        let mut builder = Request::builder().uri(uri);
        if signed_in {
            builder = builder.header(header::COOKIE, create_test_session(&sessions).await);
        }
        let response = build_router(state)
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(if signed_in {
            response.status() == StatusCode::FORBIDDEN
        } else {
            response.status().is_redirection()
        });
        assert!(
            calls.lock().unwrap().is_empty(),
            "auth refusal precedes graph upstream work"
        );
    }
}

#[tokio::test]
async fn a_lazy_graph_cannot_resolve_a_slug_from_another_project() {
    let mut fixture = behavior(vec![], vec![]);
    fixture.list_artifacts_by_project = Some(std::collections::HashMap::from([
        ("my-api".into(), vec![artifact()]),
        ("other-project".into(), vec![]),
    ]));
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(fixture),
    );
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/other-project/releases/graph-release/graph")
                .header(header::COOKIE, create_test_session(&sessions).await)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn configured_destinations_and_empty_environments_remain_unknown_without_current_state() {
    let mut release = artifact();
    release.destinations.push(ArtifactDestination {
        name: "configured-only".into(),
        environment: "prod".into(),
        type_organisation: None,
        type_name: None,
        type_version: None,
        status: None,
    });
    let mut fixture = behavior(vec![stage("old-success", "SUCCEEDED", &[])], vec![]);
    fixture.list_artifacts_result = Some(Ok(vec![release]));
    fixture.list_environments_result = Some(Ok(vec![
        forage_core::platform::Environment {
            id: "prod".into(),
            organisation: "testorg".into(),
            name: "prod".into(),
            description: None,
            sort_order: 1,
            created_at: String::new(),
        },
        forage_core::platform::Environment {
            id: "empty".into(),
            organisation: "testorg".into(),
            name: "empty".into(),
            description: None,
            sort_order: 0,
            created_at: String::new(),
        },
    ]));
    let (body, _) = request(
        fixture,
        "/api/orgs/testorg/projects/my-api/timeline?include_graph=true",
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let fragment = body["graph_html"].as_str().unwrap();
    let nodes = nodes(fragment);
    assert_eq!(nodes.len(), 3);
    assert!(nodes.iter().all(|node| node.state == "unknown"));
    assert_eq!(
        attr(nodes[0].markup, "data-node-label"),
        "empty",
        "configured environment order is retained"
    );
    let destination = nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-kind") == "destination")
        .unwrap();
    assert_eq!(
        attr(destination.markup, "data-node-label"),
        "configured-only"
    );
    assert_eq!(
        attr(destination.markup, "data-node-reason"),
        "No current destination state reported"
    );
    assert!(!fragment.contains("old-success"));
    assert_eq!(edges(fragment).len(), 1);
}

#[tokio::test]
async fn one_target_card_retains_current_and_queued_operations_and_stable_identity_across_releases()
{
    let fixture = |revision: &str| {
        let mut completed = step("completed-deploy", "prod-eu", "SUCCEEDED");
        completed.release_id = format!("finished-operation-{revision}");
        completed.completed_at = Some("2026-11-21T12:05:00Z".into());
        let mut queued = step("incoming-deploy", "prod-eu", "QUEUED");
        queued.release_id = format!("queued-operation-{revision}");
        queued.queued_at = Some("2026-11-21T12:01:00Z".into());
        queued.error_message = Some("waiting for the current release".into());
        let mut release = artifact();
        release.artifact_id = format!("artifact-{revision}");
        release.slug = format!("release-{revision}");
        release.context.title = format!("Release {revision}");
        let mut fixture = behavior(
            vec![
                stage("completed-deploy", "SUCCEEDED", &[]),
                stage("incoming-deploy", "RUNNING", &[]),
            ],
            vec![completed.clone(), queued.clone()],
        );
        fixture
            .get_release_intent_states_result
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()[0]
            .artifact_id = release.artifact_id.clone();
        fixture.get_destination_states_result = Some(Ok(DeploymentStates {
            destinations: vec![
                DestinationState {
                    artifact_id: Some(release.artifact_id.clone()),
                    release_id: Some(completed.release_id),
                    error_message: None,
                    completed_at: completed.completed_at,
                    ..current_destination("prod-eu", "SUCCEEDED")
                },
                DestinationState {
                    artifact_id: Some(release.artifact_id.clone()),
                    release_id: Some(queued.release_id),
                    queued_at: queued.queued_at,
                    error_message: queued.error_message,
                    ..current_destination("prod-eu", "QUEUED")
                },
            ],
        }));
        fixture.list_artifacts_result = Some(Ok(vec![release]));
        fixture
    };
    let uri = "/api/orgs/testorg/projects/my-api/timeline?include_graph=true";
    let (body, _) = request(fixture("first"), uri).await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let fragment = body["graph_html"].as_str().unwrap();
    assert_eq!(attr(fragment, "data-live"), "true");
    let rendered_nodes = nodes(fragment);
    let targets: Vec<_> = rendered_nodes
        .iter()
        .filter(|node| attr(node.markup, "data-node-kind") == "destination")
        .collect();
    assert_eq!(
        targets.len(),
        1,
        "operations share one physical target card"
    );
    let target = targets[0];
    assert_eq!(target.state, "waiting", "queued work remains visible");
    assert_eq!(
        attr(target.markup, "data-node-reason"),
        "waiting for the current release"
    );
    let current = node_field(target, "data-node-summary");
    let incoming = node_field(target, "data-node-preview");
    assert!(current.contains("release-first") && current.contains("Done"));
    assert!(incoming.contains("release-first") && incoming.contains("Waiting"));
    let details = attr(target.markup, "data-node-details");
    for record in [
        "finished-operation-first",
        "queued-operation-first",
        "my-api",
        "release-first",
        "Release first",
        "2026-11-21T12:05:00Z",
        "2026-11-21T12:01:00Z",
        "waiting for the current release",
    ] {
        assert!(
            details.contains(record),
            "missing operation context: {record}"
        );
    }
    let stable_key = attr(target.markup, "data-node-stable-key");
    assert!(!stable_key.is_empty());
    let environment = rendered_nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-kind") == "environment")
        .unwrap();
    assert_eq!(edges(fragment), vec![(environment.id, target.id)]);
    let (updated, _) = request(fixture("next"), uri).await;
    let updated: serde_json::Value = serde_json::from_str(&updated).unwrap();
    let updated_nodes = nodes(updated["graph_html"].as_str().unwrap());
    let updated_target = updated_nodes
        .iter()
        .find(|node| attr(node.markup, "data-node-kind") == "destination")
        .unwrap();
    assert_eq!(
        attr(updated_target.markup, "data-node-stable-key"),
        stable_key,
        "new operation IDs and release provenance do not change physical target identity"
    );
    assert_ne!(
        attr(updated_target.markup, "data-node-context"),
        attr(target.markup, "data-node-context")
    );
}

#[tokio::test]
async fn environment_lanes_wrap_targets_without_overlapping_rendered_cards_or_losing_logical_edges()
{
    let mut fixture = behavior(vec![], vec![]);
    fixture.list_environments_result = Some(Ok(["dev", "prod"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| forage_core::platform::Environment {
            id: name.into(),
            organisation: "testorg".into(),
            name: name.into(),
            description: None,
            sort_order: index as i32,
            created_at: String::new(),
        })
        .collect()));
    fixture.get_destination_states_result = Some(Ok(DeploymentStates {
        destinations: [
            ("dev", vec!["shared", "dev-west"]),
            (
                "prod",
                vec![
                    "shared", "prod-eu", "prod-us", "prod-ap", "prod-ca", "prod-au",
                ],
            ),
        ]
        .into_iter()
        .flat_map(|(environment, names)| {
            names.into_iter().map(move |name| DestinationState {
                artifact_id: Some("graph-artifact".into()),
                environment: environment.into(),
                error_message: None,
                ..current_destination(name, "RUNNING")
            })
        })
        .collect(),
    }));
    let (body, _) = request(
        fixture,
        "/api/orgs/testorg/projects/my-api/timeline?include_graph=true",
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let fragment = body["graph_html"].as_str().unwrap();
    let nodes = nodes(fragment);
    assert_eq!(
        nodes.len(),
        10,
        "two environments and eight physical targets"
    );
    let card = Regex::new(r#"<svg\b[^>]*\bclass="release-graph-card"[^>]*>"#).unwrap();
    let bounds: Vec<_> = nodes
        .iter()
        .map(|node| {
            let markup = card.find(node.markup).expect("rendered card").as_str();
            let dimensions = ["x", "y", "width", "height"]
                .map(|name| attr(markup, name).parse::<f64>().unwrap());
            (node, dimensions)
        })
        .collect();
    let canvas_width = attr(fragment, "data-graph-width").parse::<f64>().unwrap();
    let canvas_height = attr(fragment, "data-graph-height").parse::<f64>().unwrap();
    for (index, (node, [x, y, width, height])) in bounds.iter().enumerate() {
        assert!(*width > 0.0 && *height > 0.0);
        assert!(
            *x >= 0.0 && *y >= 0.0 && x + width <= canvas_width && y + height <= canvas_height,
            "card {} must remain inside the canvas",
            node.id
        );
        for (other, [other_x, other_y, other_width, other_height]) in &bounds[index + 1..] {
            assert!(
                x + width <= *other_x
                    || other_x + other_width <= *x
                    || y + height <= *other_y
                    || other_y + other_height <= *y,
                "rendered cards {} and {} overlap: {:?} and {:?}",
                node.id,
                other.id,
                [x, y, width, height],
                [other_x, other_y, other_width, other_height]
            );
        }
    }
    let edges = edges(fragment);
    assert_eq!(edges.len(), 8, "one logical environment branch per target");
    for environment in ["dev", "prod"] {
        let (parent, [_, parent_y, _, parent_height]) = bounds
            .iter()
            .find(|(node, _)| {
                attr(node.markup, "data-node-kind") == "environment"
                    && attr(node.markup, "data-node-label") == environment
            })
            .unwrap();
        let targets: Vec<_> = bounds
            .iter()
            .filter(|(node, _)| {
                attr(node.markup, "data-node-kind") == "destination"
                    && attr(node.markup, "data-node-environment") == environment
            })
            .collect();
        assert_eq!(targets.len(), if environment == "prod" { 6 } else { 2 });
        for (target, [_, target_y, _, _]) in &targets {
            assert!(
                parent_y + parent_height < *target_y,
                "environment cards sit above their destinations"
            );
            assert!(edges.contains(&(parent.id, target.id)));
            assert!(
                targets
                    .iter()
                    .filter(|(_, [_, row_y, _, _])| row_y == target_y)
                    .count()
                    <= 4,
                "destination rows wrap after at most four cards"
            );
        }
        if environment == "prod" {
            assert!(
                targets
                    .iter()
                    .any(|(_, [_, y, _, _])| *y != targets[0].1[1]),
                "more than four targets occupy multiple rows"
            );
        }
    }
    let shared_keys: HashSet<_> = nodes
        .iter()
        .filter(|node| attr(node.markup, "data-node-label") == "shared")
        .map(|node| attr(node.markup, "data-node-stable-key"))
        .collect();
    assert_eq!(
        shared_keys.len(),
        2,
        "target identity includes its environment"
    );
}

