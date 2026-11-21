use std::collections::HashSet;

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
async fn the_workspace_inspects_nodes_without_moving_existing_content_or_duplicating_actions() {
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
                < graph_start,
            "{marker} moved below graph"
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
            html[..graph_start].contains(&target),
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
async fn release_overviews_do_not_include_the_detail_graph() {
    for uri in [
        "/orgs/testorg/projects/my-api/releases",
        "/orgs/testorg/releases",
    ] {
        let (html, _) = request(behavior(vec![stage("deploy", "RUNNING", &[])], vec![]), uri).await;
        assert!(!html.contains("id=\"release-graph\""), "{uri}");
        assert!(!html.contains("data-node-id="), "{uri}");
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

fn queue(html: &str) -> &str {
    let start = html
        .find("<section id=\"release-queue\"")
        .expect("compact queue");
    let rest = &html[start..];
    &rest[..rest.find("</section>").expect("closed compact queue") + "</section>".len()]
}

#[tokio::test]
async fn the_above_fold_queue_only_shows_current_work_and_not_completed_or_future_stages() {
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
    let compact = queue(&html);
    assert!(compact.contains("Plan approval"), "{compact}");
    assert!(
        !compact.contains("data-node-label=\"complete\""),
        "{compact}"
    );
    assert!(!compact.contains("data-node-label=\"future\""), "{compact}");
    assert!(html.find(compact).unwrap() < html.find("id=\"pipeline\"").unwrap());
    assert!(
        compact.contains("data-queue-node-key="),
        "queue nodes open the full workspace"
    );
    assert!(
        html.contains("id=\"release-graph\""),
        "the full workspace remains below"
    );
}

#[tokio::test]
async fn queue_items_keep_destination_wait_reasons_and_terminal_releases_hide_the_queue() {
    let mut execution = step("deploy", "prod-eu", "QUEUED");
    execution.error_message = Some("waiting for release older to apply its plan".into());
    let (html, _) = request(
        behavior(vec![stage("deploy", "RUNNING", &[])], vec![execution]),
        PAGE,
    )
    .await;
    let compact = queue(&html);
    assert!(compact.contains("prod-eu") && compact.contains("waiting for release older"));
    let (html, _) = request(
        behavior(
            vec![stage("deploy", "SUCCEEDED", &[])],
            vec![step("deploy", "prod-eu", "SUCCEEDED")],
        ),
        PAGE,
    )
    .await;
    let compact = queue(&html);
    let opening = &compact[..compact.find('>').unwrap()];
    assert!(
        opening.contains("hidden"),
        "terminal release does not occupy the above-fold queue"
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
