use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::integrations::router::ReleaseContext;
use crate::platform::{
    ArtifactContext, ArtifactDestination, ArtifactRef, ArtifactSource, PlanDestinationOutput,
};

fn event(kind: &str, destination: &str, env: &str) -> NotificationEvent {
    NotificationEvent {
        id: "notif-1".into(),
        notification_type: kind.into(),
        title: "Release succeeded: understory/agent-gateway".into(),
        body: String::new(),
        organisation: "understory".into(),
        project: "agent-gateway".into(),
        timestamp: "2026-11-19T10:30:00Z".into(),
        release: Some(ReleaseContext {
            slug: "vivaciously-ultimate-condor".into(),
            artifact_id: "art-3".into(),
            release_intent_id: "ri-1".into(),
            destination: destination.into(),
            environment: env.into(),
            source_username: "kjuulh".into(),
            source_user_id: "u-1".into(),
            source_attribution: "actor".into(),
            commit_sha: "dc989e4aa1b2".into(),
            commit_branch: "main".into(),
            context_title: "feat(tasks): default a task to task-general (#152)".into(),
            context_web: "https://github.com/understory-io/agent-gateway/commit/dc989e4".into(),
            destination_count: 2,
            error_message: None,
        }),
    }
}

fn stage(id: &str, kind: &str, env: &str, status: &str, deps: &[&str]) -> PipelineRunStageState {
    let terminal = matches!(status, "SUCCEEDED" | "FAILED" | "CANCELLED");
    PipelineRunStageState {
        stage_id: id.into(),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        stage_type: kind.into(),
        status: status.into(),
        environment: Some(env.into()),
        duration_seconds: None,
        queued_at: Some("2026-11-19T10:29:50Z".into()),
        started_at: (status != "PENDING").then(|| "2026-11-19T10:30:00Z".into()),
        completed_at: terminal.then(|| "2026-11-19T10:31:12Z".into()),
        error_message: None,
        wait_until: None,
        release_ids: vec![],
        approval_status: None,
        auto_approve: None,
        gate_deadline: None,
        gate_waiting_on: vec![],
    }
}

fn step(stage_id: &str, dest: &str, env: &str) -> ReleaseStepState {
    ReleaseStepState {
        release_id: format!("rel-{dest}"),
        stage_id: Some(stage_id.into()),
        destination_name: dest.into(),
        environment: env.into(),
        status: String::new(),
        queued_at: None,
        assigned_at: None,
        started_at: None,
        completed_at: None,
        error_message: None,
    }
}

fn dest(env: &str, status: &str) -> DestinationStatus {
    DestinationStatus {
        environment: env.into(),
        status: status.into(),
        error: None,
        started_at: Some("2026-11-19T10:30:00Z".into()),
        completed_at: (status != "started").then(|| "2026-11-19T10:31:05Z".into()),
    }
}

fn steps() -> Vec<ReleaseStepState> {
    vec![
        step("dev", "dev-k8s", "dev"),
        step("prod", "prod-k8s", "prod"),
    ]
}

struct Case {
    event: NotificationEvent,
    destinations: HashMap<String, DestinationStatus>,
    stages: Vec<PipelineRunStageState>,
    steps: Vec<ReleaseStepState>,
    emoji: SlackEmojiSet,
    expected: usize,
}

impl Case {
    fn new(event: NotificationEvent, stages: Vec<PipelineRunStageState>) -> Self {
        let expected = event.release.as_ref().map_or(0, |r| r.destination_count as usize);
        Self {
            expected,
            event,
            destinations: HashMap::new(),
            stages,
            steps: steps(),
            emoji: SlackEmojiSet::default(),
        }
    }

    fn with(mut self, name: &str, d: DestinationStatus) -> Self {
        self.destinations.insert(name.into(), d);
        self
    }

    fn view(&self) -> ReleaseView<'_> {
        ReleaseView {
            event: &self.event,
            destinations: &self.destinations,
            stages: &self.stages,
            steps: &self.steps,
            forage_url: "https://forest.understory.sh",
            emoji: &self.emoji,
            expected_destinations: self.expected,
        }
    }

    fn parent(&self) -> Value {
        let m = render_parent(&self.view());
        json!({ "text": m.text, "blocks": m.blocks })
    }

    fn header(&self) -> String {
        render_parent(&self.view()).blocks[0]["text"]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[test]
fn queued_release_renders_every_stage_as_queued() {
    let case = Case::new(
        event("release_annotated", "", ""),
        vec![
            stage("dev", "deploy", "dev", "PENDING", &[]),
            stage("prod", "deploy", "prod", "PENDING", &["dev"]),
        ],
    );
    assert_eq!(overall_state(&case.view()), OverallState::InProgress);
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn running_release_shows_the_spinner_and_start_time() {
    let case = Case::new(
        event("release_started", "dev-k8s", "dev"),
        vec![
            stage("dev", "deploy", "dev", "RUNNING", &[]),
            stage("prod", "deploy", "prod", "PENDING", &["dev"]),
        ],
    )
    .with("dev-k8s", dest("dev", "started"));
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn header_does_not_claim_success_while_a_stage_is_still_running() {
    let case = Case::new(
        event("release_succeeded", "dev-k8s", "dev"),
        vec![
            stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
            stage("prod", "deploy", "prod", "RUNNING", &["dev"]),
        ],
    )
    .with("dev-k8s", dest("dev", "succeeded"))
    .with("prod-k8s", dest("prod", "started"));

    assert_eq!(overall_state(&case.view()), OverallState::InProgress);
    let header = case.header();
    assert!(
        header.contains("Releasing understory/agent-gateway"),
        "{header}"
    );
    assert!(!header.to_lowercase().contains("succeeded"), "{header}");
    assert!(!header.contains("Released"), "{header}");
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn header_does_not_claim_success_without_a_pipeline_until_every_destination_reported() {
    let mut case = Case::new(event("release_succeeded", "dev-k8s", "dev"), vec![])
        .with("dev-k8s", dest("dev", "succeeded"));
    case.steps.clear();

    assert_eq!(overall_state(&case.view()), OverallState::InProgress);
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn waiting_for_approval_release() {
    let case = Case::new(
        event("release_succeeded", "dev-k8s", "dev"),
        vec![
            stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
            stage("plan-prod", "plan", "prod", "AWAITING_APPROVAL", &["dev"]),
            stage("prod", "deploy", "prod", "PENDING", &["plan-prod"]),
        ],
    )
    .with("dev-k8s", dest("dev", "succeeded"));
    assert_eq!(
        overall_state(&case.view()),
        OverallState::WaitingForApproval
    );
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn succeeded_release_shows_total_duration() {
    let mut stages = vec![
        stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
        stage("prod", "deploy", "prod", "SUCCEEDED", &["dev"]),
    ];
    stages[1].started_at = Some("2026-11-19T10:31:20Z".into());
    stages[1].completed_at = Some("2026-11-19T10:34:10Z".into());
    let case = Case::new(event("release_succeeded", "prod-k8s", "prod"), stages)
        .with("dev-k8s", dest("dev", "succeeded"))
        .with("prod-k8s", dest("prod", "succeeded"));
    assert_eq!(overall_state(&case.view()), OverallState::Succeeded);
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn failed_release() {
    let mut stages = vec![
        stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
        stage("prod", "deploy", "prod", "FAILED", &["dev"]),
    ];
    stages[1].error_message = Some("rollout timed out".into());
    let mut failed = dest("prod", "failed");
    failed.error = Some("rollout timed out".into());
    let case = Case::new(event("release_failed", "prod-k8s", "prod"), stages)
        .with("dev-k8s", dest("dev", "succeeded"))
        .with("prod-k8s", failed);
    assert_eq!(overall_state(&case.view()), OverallState::Failed);
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn cancelled_and_skipped_stages() {
    let case = Case::new(
        event("release_failed", "", ""),
        vec![
            stage("dev", "deploy", "dev", "CANCELLED", &[]),
            stage("prod", "deploy", "prod", "CANCELLED", &["dev"]),
        ],
    );
    assert_eq!(overall_state(&case.view()), OverallState::Cancelled);
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn a_destination_event_ahead_of_the_pipeline_finishes_its_stage() {
    let case = Case::new(
        event("release_succeeded", "prod-k8s", "prod"),
        vec![
            stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
            stage("prod", "deploy", "prod", "RUNNING", &["dev"]),
        ],
    )
    .with("dev-k8s", dest("dev", "succeeded"))
    .with("prod-k8s", dest("prod", "succeeded"));
    assert_eq!(overall_state(&case.view()), OverallState::Succeeded);
}

#[test]
fn org_emoji_set_replaces_the_standard_emoji() {
    let mut case = Case::new(
        event("release_started", "prod-k8s", "prod"),
        vec![
            stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
            stage("prod", "deploy", "prod", "RUNNING", &["dev"]),
            stage("e2e", "deploy", "e2e", "PENDING", &["prod"]),
        ],
    );
    case.emoji = SlackEmojiSet {
        queued: "loading".into(),
        running: "sonicfastafboi".into(),
        celebrate: "party-parrot".into(),
        ..Default::default()
    };
    let text = case.parent().to_string();
    assert!(text.contains(":sonicfastafboi: *Releasing"), "{text}");
    assert!(text.contains(":loading:  Deploy to `e2e`"), "{text}");
    assert!(
        text.contains(":white_check_mark:  Deployed to `dev`"),
        "{text}"
    );
    insta::assert_json_snapshot!(case.parent());
}

#[test]
fn emoji_names_are_normalised_and_validated() {
    let mut fields = BTreeMap::new();
    fields.insert("running".to_string(), ":sonicfastafboi:".to_string());
    fields.insert("celebrate".to_string(), " party-parrot ".to_string());
    let set = SlackEmojiSet::from_fields(&fields).unwrap();
    assert_eq!(set.running, "sonicfastafboi");
    assert_eq!(set.get("running"), ":sonicfastafboi:");
    assert_eq!(set.get("celebrate"), ":party-parrot:");
    assert_eq!(set.get("failed"), ":x:");

    fields.insert("failed".to_string(), "<script>".to_string());
    assert!(SlackEmojiSet::from_fields(&fields).is_err());
}

#[test]
fn plan_summary_reads_terraform_output() {
    assert_eq!(
        plan_summary("...\nPlan: 1 to add, 2 to change, 3 to destroy.\n"),
        Some((1, 2, 3))
    );
    assert_eq!(
        plan_summary("No changes. Your infrastructure matches."),
        Some((0, 0, 0))
    );
    assert_eq!(
        plan_summary("Plan: 4 to add, 0 to change, 0 to destroy."),
        Some((4, 0, 0))
    );
    assert_eq!(plan_summary("something else"), None);
}

fn artifact(id: &str, created: &str, title: &str, deployed: bool) -> Artifact {
    Artifact {
        artifact_id: id.into(),
        slug: format!("slug-{id}"),
        context: ArtifactContext {
            title: title.into(),
            description: None,
            web: Some(format!("https://github.com/o/r/commit/{id}")),
            pr: (id == "art-2").then(|| "https://github.com/o/r/pull/151".into()),
        },
        source: Some(ArtifactSource {
            user: Some("kjuulh".into()),
            email: None,
            source_type: None,
            run_url: None,
        }),
        git_ref: Some(ArtifactRef {
            commit_sha: format!("{id}0000000"),
            branch: Some("main".into()),
            commit_message: None,
            version: None,
            repo_url: None,
        }),
        destinations: vec![ArtifactDestination {
            name: "prod-k8s".into(),
            environment: "prod".into(),
            type_organisation: None,
            type_name: None,
            type_version: None,
            status: Some(if deployed { "SUCCEEDED" } else { "QUEUED" }.into()),
        }],
        created_at: created.into(),
    }
}

#[test]
fn thread_posts_for_a_failed_release_with_plan_and_changes() {
    let mut stages = vec![
        stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
        stage("plan-prod", "plan", "prod", "SUCCEEDED", &["dev"]),
        stage("prod", "deploy", "prod", "FAILED", &["plan-prod"]),
    ];
    stages[1].approval_status = Some("approved".into());
    stages[2].error_message = Some("Error: rollout of deployment/agent-gateway timed out".into());
    let mut failed = dest("prod", "failed");
    failed.error = Some("Error: rollout of deployment/agent-gateway timed out".into());
    let case = Case::new(event("release_failed", "prod-k8s", "prod"), stages)
        .with("dev-k8s", dest("dev", "succeeded"))
        .with("prod-k8s", failed);

    let artifacts = vec![
        artifact(
            "art-0",
            "2026-11-18T09:00:00Z",
            "older, deployed before",
            true,
        ),
        artifact("art-1", "2026-11-19T08:00:00Z", "fix: retry on 503", false),
        artifact("art-2", "2026-11-19T09:00:00Z", "feat: task caps", false),
        artifact(
            "art-3",
            "2026-11-19T10:00:00Z",
            "feat(tasks): default a task",
            false,
        ),
    ];
    let plan = PlanOutput {
        plan_output: String::new(),
        status: "SUCCEEDED".into(),
        outputs: vec![PlanDestinationOutput {
            destination_id: "d-1".into(),
            destination_name: "prod-k8s".into(),
            plan_output: "Plan: 1 to add, 0 to change, 2 to destroy.".into(),
            status: "SUCCEEDED".into(),
        }],
    };
    let detail = ThreadDetail {
        artifacts: Some(&artifacts),
        plans: HashMap::from([("plan-prod".to_string(), &plan)]),
    };

    let posts = thread_posts(&case.view(), &detail);
    let keys: Vec<&str> = posts.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "changes",
            "plan:plan-prod",
            "approval:plan-prod:approved",
            "deploy:dev-k8s",
            "failure:prod-k8s",
        ]
    );
    assert!(
        posts[0]
            .text
            .contains("3 changes since the previous release")
    );
    assert!(!posts[0].text.contains("older, deployed before"));
    assert!(posts[1].text.contains("destroys 2 resources"));
    insta::assert_json_snapshot!(posts);
}

#[test]
fn approval_request_names_who_can_approve_and_reaches_direct_messages() {
    let case = Case::new(
        event("release_succeeded", "dev-k8s", "dev"),
        vec![
            stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
            stage("plan-prod", "plan", "prod", "AWAITING_APPROVAL", &["dev"]),
        ],
    );
    let posts = thread_posts(&case.view(), &ThreadDetail::default());
    let approval = posts
        .iter()
        .find(|p| p.key == "approval:plan-prod:requested")
        .expect("approval post");
    assert!(
        approval
            .text
            .contains("Any member of *understory* can approve")
    );
    assert!(approval.for_direct_message());
    assert!(!ThreadPost::new("changes".into(), String::new()).for_direct_message());
}

#[test]
fn a_single_change_posts_no_changes_reply() {
    let case = Case::new(event("release_started", "dev-k8s", "dev"), vec![]);
    let artifacts = vec![
        artifact("art-2", "2026-11-19T09:00:00Z", "deployed", true),
        artifact("art-3", "2026-11-19T10:00:00Z", "this one", false),
    ];
    let detail = ThreadDetail {
        artifacts: Some(&artifacts),
        ..Default::default()
    };
    assert!(thread_posts(&case.view(), &detail).is_empty());
}

#[test]
fn success_event_without_a_count_does_not_finish_a_release_the_start_event_sized() {
    let mut e = event("release_succeeded", "dev-k8s", "dev");
    e.release.as_mut().unwrap().destination_count = 0;
    let mut case = Case::new(e, vec![]).with("dev-k8s", dest("dev", "succeeded"));
    case.steps.clear();
    case.expected = 2;
    assert_eq!(overall_state(&case.view()), OverallState::InProgress);
}

#[test]
fn release_steps_list_destinations_no_event_has_named_yet() {
    let mut e = event("release_succeeded", "dev-k8s", "dev");
    e.release.as_mut().unwrap().destination_count = 0;
    let mut case = Case::new(e, vec![]).with("dev-k8s", dest("dev", "succeeded"));
    case.expected = 0;
    assert_eq!(overall_state(&case.view()), OverallState::InProgress);
    let text = case.parent().to_string();
    assert!(text.contains("Deploy `prod-k8s`"), "{text}");
}

#[test]
fn a_superseded_stage_finishes_the_release() {
    let case = Case::new(
        event("release_succeeded", "dev-k8s", "dev"),
        vec![
            stage("dev", "deploy", "dev", "SUCCEEDED", &[]),
            stage("prod", "deploy", "prod", "SUPERSEDED", &["dev"]),
        ],
    )
    .with("dev-k8s", dest("dev", "succeeded"));
    assert_eq!(overall_state(&case.view()), OverallState::Succeeded);
    let text = case.parent().to_string();
    assert!(text.contains("Deploy to `prod` skipped"), "{text}");
    assert!(text.contains("1 skipped"), "{text}");
}

#[test]
fn outside_text_is_escaped_so_it_cannot_ping_or_forge_links() {
    let mut e = event("release_failed", "prod-k8s", "prod");
    {
        let r = e.release.as_mut().unwrap();
        r.context_title = "fix <!channel> & <https://evil.example|PR>".into();
        r.commit_sha = "dé9ç".repeat(4);
        r.context_web = "javascript:alert(1)".into();
    }
    let mut failed = dest("prod", "failed");
    failed.error = Some("<!here> boom".into());
    let case = Case::new(e, vec![]).with("prod-k8s", failed);
    let parent = case.parent().to_string();
    assert!(parent.contains("fix &lt;!channel&gt; &amp; &lt;https://evil.example|PR&gt;"), "{parent}");
    assert!(!parent.contains("javascript:"), "{parent}");
    let posts = thread_posts(&case.view(), &ThreadDetail::default());
    let failure = posts.iter().find(|p| p.key == "failure:prod-k8s").unwrap();
    assert!(failure.text.contains("&lt;!here&gt; boom"), "{}", failure.text);
}
