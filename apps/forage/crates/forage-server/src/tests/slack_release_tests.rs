use std::sync::{Arc, Mutex};

use forage_core::integrations::router::{
    DispatchTask, NotificationEvent, ReleaseContext, SlackMessage,
};
use forage_core::integrations::slack_release::SlackEmojiSet;
use forage_core::integrations::{
    CreateIntegrationInput, InMemoryIntegrationStore, IntegrationConfig, IntegrationStore,
    IntegrationType,
};
use forage_core::platform::{
    Artifact, ArtifactContext, ArtifactDestination, PipelineRunStageState, PlanOutput,
    ReleaseStepState,
};
use serde_json::Value;

use crate::notification_worker::{NotificationDispatcher, ReleaseDetailSource};
use crate::test_providers::FakeProviders;

#[derive(Default)]
struct FakeDetails {
    stages: Mutex<Vec<PipelineRunStageState>>,
    artifact_calls: Mutex<u32>,
    pipeline_failures: Mutex<u32>,
    only_one_change: Mutex<bool>,
}

#[async_trait::async_trait]
impl ReleaseDetailSource for FakeDetails {
    async fn pipeline(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<Option<(Vec<PipelineRunStageState>, Vec<ReleaseStepState>)>, String> {
        let steps = ["dev", "prod"]
            .iter()
            .map(|env| ReleaseStepState {
                release_id: format!("rel-{env}"),
                stage_id: Some(env.to_string()),
                destination_name: format!("{env}-k8s"),
                environment: env.to_string(),
                status: String::new(),
                queued_at: None,
                assigned_at: None,
                started_at: None,
                completed_at: None,
                error_message: None,
            })
            .collect();
        {
            let mut failures = self.pipeline_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err("forest unavailable".into());
            }
        }
        Ok(Some((self.stages.lock().unwrap().clone(), steps)))
    }

    async fn artifacts(&self, _: &str, _: &str) -> Option<Vec<Artifact>> {
        *self.artifact_calls.lock().unwrap() += 1;
        let artifact = |id: &str, created: &str, deployed: bool| Artifact {
            artifact_id: id.into(),
            slug: format!("slug-{id}"),
            context: ArtifactContext {
                title: format!("change {id}"),
                description: None,
                web: None,
                pr: None,
            },
            source: None,
            git_ref: None,
            destinations: vec![ArtifactDestination {
                name: "prod-k8s".into(),
                environment: "prod".into(),
                type_organisation: None,
                type_name: None,
                type_version: None,
                status: deployed.then(|| "SUCCEEDED".into()),
            }],
            created_at: created.into(),
        };
        if *self.only_one_change.lock().unwrap() {
            return Some(vec![
                artifact("art-2", "2026-11-19T09:00:00Z", true),
                artifact("art-3", "2026-11-19T10:00:00Z", false),
            ]);
        }
        Some(vec![
            artifact("art-1", "2026-11-18T10:00:00Z", true),
            artifact("art-2", "2026-11-19T09:00:00Z", false),
            artifact("art-3", "2026-11-19T10:00:00Z", false),
        ])
    }

    async fn plan_output(&self, _: &str, _: &str) -> Option<PlanOutput> {
        None
    }
}

fn stage(id: &str, status: &str, deps: &[&str]) -> PipelineRunStageState {
    PipelineRunStageState {
        stage_id: id.into(),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        stage_type: "deploy".into(),
        status: status.into(),
        environment: Some(id.into()),
        duration_seconds: None,
        queued_at: None,
        started_at: (status != "PENDING").then(|| "2026-11-19T10:30:00Z".into()),
        completed_at: (status == "SUCCEEDED").then(|| "2026-11-19T10:31:00Z".into()),
        error_message: None,
        wait_until: None,
        release_ids: vec![],
        approval_status: None,
        auto_approve: None,
        gate_deadline: None,
        gate_waiting_on: vec![],
    }
}

fn event(kind: &str, env: &str) -> NotificationEvent {
    NotificationEvent {
        id: format!("n-{kind}-{env}"),
        notification_type: kind.into(),
        title: format!("{kind}: understory/agent-gateway"),
        body: String::new(),
        organisation: "understory".into(),
        project: "agent-gateway".into(),
        timestamp: "2026-11-19T10:31:00Z".into(),
        release: Some(ReleaseContext {
            slug: "vivaciously-ultimate-condor".into(),
            artifact_id: "art-3".into(),
            release_intent_id: "ri-1".into(),
            destination: format!("{env}-k8s"),
            environment: env.into(),
            source_username: "kjuulh".into(),
            source_user_id: "u-1".into(),
            source_attribution: "actor".into(),
            commit_sha: "dc989e4".into(),
            commit_branch: "main".into(),
            context_title: "feat(tasks): default a task".into(),
            context_web: String::new(),
            destination_count: 2,
            error_message: (kind == "release_failed").then(|| "rollout timed out".into()),
        }),
    }
}

struct Harness {
    providers: FakeProviders,
    store: Arc<InMemoryIntegrationStore>,
    details: Arc<FakeDetails>,
    dispatcher: NotificationDispatcher,
    integration_id: String,
}

impl Harness {
    async fn new() -> Self {
        let providers = FakeProviders::start().await;
        let store = Arc::new(InMemoryIntegrationStore::new());
        let integration = store
            .create_integration(&CreateIntegrationInput {
                organisation: "understory".into(),
                integration_type: IntegrationType::Slack,
                name: "#deploys".into(),
                config: IntegrationConfig::Slack {
                    team_id: "T1".into(),
                    team_name: "Understory".into(),
                    channel_id: "C1".into(),
                    channel_name: "#deploys".into(),
                    access_token: "xoxb-test".into(),
                    webhook_url: String::new(),
                },
                created_by: "u-1".into(),
            })
            .await
            .unwrap();
        let details = Arc::new(FakeDetails::default());
        let dispatcher =
            NotificationDispatcher::new(store.clone(), "https://forest.example".into())
                .with_slack_api(providers.slack_api())
                .with_release_details(details.clone());
        Self {
            providers,
            store,
            details,
            dispatcher,
            integration_id: integration.id,
        }
    }

    fn set_stages(&self, stages: Vec<PipelineRunStageState>) {
        *self.details.stages.lock().unwrap() = stages;
    }

    async fn send(&self, kind: &str, env: &str) {
        let event = event(kind, env);
        self.dispatcher
            .dispatch(&DispatchTask::Slack {
                integration_id: self.integration_id.clone(),
                webhook_url: String::new(),
                access_token: "xoxb-test".into(),
                channel_id: "C1".into(),
                release_id: "vivaciously-ultimate-condor".into(),
                notification_id: event.id.clone(),
                event_type: kind.into(),
                event,
                message: SlackMessage {
                    text: String::new(),
                    color: String::new(),
                    blocks: vec![],
                },
            })
            .await;
    }

    fn parents(&self) -> Vec<Value> {
        let st = self.providers.state.lock().unwrap();
        st.post_bodies
            .iter()
            .filter(|b| b.get("thread_ts").is_none())
            .cloned()
            .collect()
    }

    fn thread(&self) -> Vec<String> {
        let st = self.providers.state.lock().unwrap();
        st.post_bodies
            .iter()
            .filter(|b| b.get("thread_ts").is_some())
            .map(|b| b["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn last_header(&self) -> String {
        let st = self.providers.state.lock().unwrap();
        let last = st
            .update_bodies
            .last()
            .or_else(|| st.post_bodies.iter().find(|b| b.get("thread_ts").is_none()))
            .unwrap();
        last["blocks"][0]["text"]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[tokio::test]
async fn a_release_keeps_one_parent_message_and_threads_the_detail_once() {
    let h = Harness::new().await;

    h.set_stages(vec![
        stage("dev", "RUNNING", &[]),
        stage("prod", "PENDING", &["dev"]),
    ]);
    h.send("release_started", "dev").await;

    h.set_stages(vec![
        stage("dev", "SUCCEEDED", &[]),
        stage("prod", "RUNNING", &["dev"]),
    ]);
    h.send("release_succeeded", "dev").await;
    assert!(
        h.last_header()
            .contains("Releasing understory/agent-gateway"),
        "dev finishing must not mark the release succeeded: {}",
        h.last_header()
    );

    h.send("release_started", "prod").await;
    h.set_stages(vec![
        stage("dev", "SUCCEEDED", &[]),
        stage("prod", "SUCCEEDED", &["dev"]),
    ]);
    h.send("release_succeeded", "prod").await;
    assert!(
        h.last_header()
            .contains("Released understory/agent-gateway"),
        "{}",
        h.last_header()
    );

    let parents = h.parents();
    assert_eq!(parents.len(), 1, "one parent message per release");
    assert!(parents[0].get("attachments").is_none());

    let thread = h.thread();
    assert_eq!(
        thread
            .iter()
            .filter(|t| t.contains("2 changes since"))
            .count(),
        1,
        "{thread:?}"
    );
    assert_eq!(
        thread
            .iter()
            .filter(|t| t.contains("|dev-k8s> (dev)"))
            .count(),
        1
    );
    assert_eq!(
        thread
            .iter()
            .filter(|t| t.contains("|prod-k8s> (prod)"))
            .count(),
        1
    );
    assert_eq!(
        *h.details.artifact_calls.lock().unwrap(),
        1,
        "artifacts fetched once"
    );

    let updates = h.providers.state.lock().unwrap().update_bodies.len();
    let posts = h.providers.state.lock().unwrap().post_bodies.len();
    h.send("release_succeeded", "prod").await;
    assert_eq!(
        h.providers.state.lock().unwrap().update_bodies.len(),
        updates,
        "redelivery is a no-op"
    );
    assert_eq!(h.providers.state.lock().unwrap().post_bodies.len(), posts);
}

#[tokio::test]
async fn rate_limited_posts_are_retried_after_the_requested_wait() {
    let h = Harness::new().await;
    h.providers.state.lock().unwrap().rate_limit_next = 1;
    h.set_stages(vec![stage("dev", "RUNNING", &[])]);
    h.send("release_started", "dev").await;
    assert_eq!(h.parents().len(), 1);
}

#[tokio::test]
async fn failure_details_go_to_the_thread() {
    let h = Harness::new().await;
    let mut prod = stage("prod", "FAILED", &["dev"]);
    prod.completed_at = Some("2026-11-19T10:32:00Z".into());
    h.set_stages(vec![stage("dev", "SUCCEEDED", &[]), prod]);
    h.send("release_failed", "prod").await;

    assert!(
        h.last_header().contains("Release failed"),
        "{}",
        h.last_header()
    );
    let thread = h.thread();
    assert!(
        thread
            .iter()
            .any(|t| t.contains("failed*") && t.contains("rollout timed out")),
        "{thread:?}"
    );
}

#[tokio::test]
async fn org_emoji_set_is_used_for_the_parent_message() {
    let h = Harness::new().await;
    h.store
        .set_slack_emoji_set(
            "understory",
            &SlackEmojiSet {
                running: "sonicfastafboi".into(),
                ..Default::default()
            },
            "u-1",
        )
        .await
        .unwrap();
    h.set_stages(vec![stage("dev", "RUNNING", &[])]);
    h.send("release_started", "dev").await;
    assert!(
        h.last_header().starts_with(":sonicfastafboi: "),
        "{}",
        h.last_header()
    );
}

#[tokio::test]
async fn a_failed_pipeline_fetch_is_retried_instead_of_rendering_without_it() {
    let h = Harness::new().await;
    *h.details.pipeline_failures.lock().unwrap() = 1;
    h.set_stages(vec![stage("dev", "SUCCEEDED", &[]), stage("prod", "RUNNING", &["dev"])]);
    h.send("release_succeeded", "dev").await;
    assert_eq!(h.parents().len(), 1);
    assert!(h.last_header().contains("Releasing"), "{}", h.last_header());
}

#[tokio::test]
async fn concurrent_events_for_one_release_share_one_parent_message() {
    let h = Harness::new().await;
    h.set_stages(vec![stage("dev", "RUNNING", &[]), stage("prod", "RUNNING", &[])]);
    tokio::join!(
        h.send("release_started", "dev"),
        h.send("release_started", "prod"),
        h.send("release_succeeded", "dev"),
    );
    assert_eq!(h.parents().len(), 1);
}

#[tokio::test]
async fn a_late_start_event_does_not_undo_a_finished_destination() {
    let h = Harness::new().await;
    h.set_stages(vec![]);
    h.send("release_succeeded", "dev").await;
    h.send("release_started", "dev").await;
    let r = h
        .store
        .get_slack_message_ref(&h.integration_id, "vivaciously-ultimate-condor")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.destinations["dev-k8s"].status, "succeeded");
}

#[tokio::test]
async fn the_artifact_list_is_fetched_once_even_when_there_is_nothing_to_list() {
    let h = Harness::new().await;
    *h.details.only_one_change.lock().unwrap() = true;
    h.set_stages(vec![stage("dev", "RUNNING", &[])]);
    h.send("release_started", "dev").await;
    h.set_stages(vec![stage("dev", "SUCCEEDED", &[])]);
    h.send("release_succeeded", "dev").await;
    assert_eq!(*h.details.artifact_calls.lock().unwrap(), 1);
    assert!(!h.thread().iter().any(|t| t.contains("changes since")));
}

#[tokio::test]
async fn forest_links_are_never_unfurled_into_sign_in_previews() {
    let h = Harness::new().await;
    h.set_stages(vec![stage("dev", "RUNNING", &[]), stage("prod", "PENDING", &["dev"])]);
    h.send("release_started", "dev").await;
    h.set_stages(vec![stage("dev", "SUCCEEDED", &[]), stage("prod", "RUNNING", &["dev"])]);
    h.send("release_succeeded", "dev").await;

    let st = h.providers.state.lock().unwrap();
    assert!(!st.update_bodies.is_empty());
    for body in st.post_bodies.iter().chain(st.update_bodies.iter()) {
        assert_eq!(body["unfurl_links"], false, "{body}");
        assert_eq!(body["unfurl_media"], false, "{body}");
    }
}
