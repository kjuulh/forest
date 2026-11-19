use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    CreatePolicyInput, Environment, PipelineStage, PipelineStageConfig, Policy, PolicyConfig,
    ReleasePipeline, UpdatePolicyInput, UpdateReleasePipelineInput,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const PROJECT: &str = "/orgs/testorg/projects/my-api";
const SOAK_ID: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const BRANCH_ID: &str = "aaaaaaaa-0000-0000-0000-000000000002";
const APPROVAL_ID: &str = "aaaaaaaa-0000-0000-0000-000000000003";
const SUPERSEDE_ID: &str = "aaaaaaaa-0000-0000-0000-000000000004";

fn policy(id: &str, name: &str, enabled: bool, config: PolicyConfig) -> Policy {
    Policy {
        id: id.into(),
        name: name.into(),
        enabled,
        policy_type: String::new(),
        config,
        created_at: "2026-03-08T00:00:00Z".into(),
        updated_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn policies() -> Vec<Policy> {
    vec![
        policy(
            SOAK_ID,
            "staging-soak-30m",
            true,
            PolicyConfig::SoakTime {
                source_environment: "staging".into(),
                target_environment: "prod".into(),
                duration_seconds: 5400,
            },
        ),
        policy(
            BRANCH_ID,
            "main-only",
            false,
            PolicyConfig::BranchRestriction {
                target_environment: "prod".into(),
                branch_pattern: "release/<v>".into(),
            },
        ),
        policy(
            APPROVAL_ID,
            "prod-approval",
            true,
            PolicyConfig::Approval {
                target_environment: "prod".into(),
                required_approvals: 2,
            },
        ),
        policy(
            SUPERSEDE_ID,
            "collapse-prod",
            true,
            PolicyConfig::SupersedePending {
                target_environment: "prod".into(),
                same_branch_only: true,
            },
        ),
    ]
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

fn pipelines() -> Vec<ReleasePipeline> {
    vec![
        ReleasePipeline {
            id: "p1".into(),
            name: "standard".into(),
            enabled: true,
            stages: vec![
                PipelineStage {
                    id: "dev".into(),
                    depends_on: vec![],
                    config: PipelineStageConfig::Deploy {
                        environment: "dev</textarea><b>".into(),
                    },
                },
                PipelineStage {
                    id: "soak".into(),
                    depends_on: vec!["dev".into()],
                    config: PipelineStageConfig::Wait {
                        duration_seconds: 60,
                    },
                },
            ],
            created_at: "2026-03-08T00:00:00Z".into(),
            updated_at: "2026-03-08T00:00:00Z".into(),
        },
        ReleasePipeline {
            id: "p2".into(),
            name: "empty".into(),
            enabled: false,
            stages: vec![],
            created_at: "2026-03-08T00:00:00Z".into(),
            updated_at: "2026-03-08T00:00:00Z".into(),
        },
    ]
}

type Recorded<T> = Arc<Mutex<Vec<T>>>;

struct App {
    router: axum::Router,
    cookie: String,
    created: Recorded<CreatePolicyInput>,
    updated: Recorded<(String, UpdatePolicyInput)>,
    pipeline_updates: Recorded<(String, UpdateReleasePipelineInput)>,
}

async fn app_with(admin: bool, policies: Vec<Policy>, pipelines: Vec<ReleasePipeline>) -> App {
    let created = Arc::new(Mutex::new(Vec::new()));
    let updated = Arc::new(Mutex::new(Vec::new()));
    let pipeline_updates = Arc::new(Mutex::new(Vec::new()));
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_policies_result: Some(Ok(policies)),
        list_release_pipelines_result: Some(Ok(pipelines)),
        list_environments_result: Some(Ok(vec![
            environment("dev"),
            environment("staging"),
            environment("prod"),
        ])),
        create_policy_inputs: Some(created.clone()),
        update_policy_inputs: Some(updated.clone()),
        update_release_pipeline_inputs: Some(pipeline_updates.clone()),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = if admin {
        create_test_session(&sessions).await
    } else {
        create_test_session_member(&sessions).await
    };
    App {
        router: build_router(state),
        cookie,
        created,
        updated,
        pipeline_updates,
    }
}

async fn app(admin: bool) -> App {
    app_with(admin, policies(), pipelines()).await
}

impl App {
    async fn get(&self, uri: &str) -> (StatusCode, String) {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("cookie", &self.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    async fn post(&self, uri: &str, form: &str) -> (StatusCode, Option<String>) {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", &self.cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let location = response
            .headers()
            .get("location")
            .map(|v| v.to_str().unwrap().to_string());
        (response.status(), location)
    }
}

fn main_element(html: &str) -> &str {
    let start = html.find("<main").expect("a <main>");
    let end = html.find("</main>").expect("a </main>") + "</main>".len();
    &html[start..end]
}

fn masked_main(html: &str) -> String {
    regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(main_element(html), "?v=HASH")
        .into_owned()
}

fn assert_clean(html: &str) {
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    assert!(!html.contains("onsubmit"), "no inline handlers: {html}");
    assert!(!html.contains("onclick"), "no inline handlers: {html}");
    assert!(!html.contains("style="), "no inline styles: {html}");
    assert!(
        !main_element(html).contains("none<"),
        "no none leak: {html}"
    );
}

fn element<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {html}"));
    let start = html[..at].rfind('<').unwrap();
    let end = at + html[at..].find('>').unwrap() + 1;
    &html[start..end]
}

fn section<'a>(html: &'a str, id: &str) -> &'a str {
    let at = html.find(&format!("id=\"{id}\"")).unwrap();
    let rest = &html[html[..at].rfind('<').unwrap()..];
    let end = [
        "soak-time-fields",
        "branch-restriction-fields",
        "approval-fields",
        "supersede-pending-fields",
    ]
    .iter()
    .filter(|other| **other != id)
    .filter_map(|other| rest.find(&format!("id=\"{other}\"")))
    .filter_map(|at| rest[..at].rfind('<'))
    .min()
    .unwrap_or_else(|| rest.find("Create Policy</button>").unwrap());
    &rest[..end]
}

fn controls(section: &str) -> Vec<&str> {
    regex::Regex::new(r"<(input|select)\b[^>]*>")
        .unwrap()
        .find_iter(section)
        .map(|m| m.as_str())
        .collect()
}

#[tokio::test]
async fn the_policies_page_lists_every_policy_type_with_its_actions() {
    let app = app(true).await;
    let (status, html) = app.get(&format!("{PROJECT}/settings/policies")).await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    for (label, detail) in [
        ("Soak Time", "1h 30m"),
        ("Branch Restriction", "release/&lt;v&gt;"),
        ("Approval Required", "2 approvals"),
        ("Supersede Pending", "newest wins, same branch only"),
    ] {
        assert!(html.contains(label), "{label}");
        assert!(html.contains(detail), "{detail}");
    }
    let base = format!("{PROJECT}/settings/policies/{SOAK_ID}");
    assert!(html.contains(&format!("href=\"{base}\"")));
    assert!(html.contains(&format!("<form method=\"post\" action=\"{base}/toggle\">")));
    assert!(html.contains(&format!(
        "<form method=\"post\" action=\"{base}/delete\" data-confirm=\"Delete policy &quot;staging-soak-30m&quot;?\">"
    )));
    assert!(html.contains("<input type=\"hidden\" name=\"csrf_token\" value=\"test-csrf\">"));
    assert!(!main_element(&html).contains("name=\"_csrf\""));
    assert!(
        html.contains(&format!(
            "action=\"{PROJECT}/settings/policies/{BRANCH_ID}/toggle\">\n<input type=\"hidden\" name=\"csrf_token\" value=\"test-csrf\">\n<input type=\"hidden\" name=\"enabled\" value=\"true\">"
        )),
        "a disabled policy posts enabled=true: {html}"
    );
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn the_create_policy_form_submits_only_the_selected_type() {
    let app = app(true).await;
    let (_, html) = app.get(&format!("{PROJECT}/settings/policies")).await;
    assert!(html.contains(&format!(
        "<form method=\"post\" action=\"{PROJECT}/settings/policies\" class=\"space-y-4\" id=\"policy-form\">"
    )));
    assert!(html.contains("name=\"policy_type\""));
    assert!(html.contains("<option value=\"soak_time\" data-description="));
    assert!(html.contains("<option value=\"supersede_pending\" data-description="));
    assert!(html.contains("<script src=\"/static/js/policy-form.js?v="));

    let soak = section(&html, "soak-time-fields");
    assert!(!element(soak, "id=\"soak-time-fields\"").contains("hidden"));
    let soak_controls = controls(soak);
    assert_eq!(soak_controls.len(), 3, "{soak}");
    for control in soak_controls {
        assert!(!control.contains("disabled"), "{control}");
    }
    for name in [
        "source_environment",
        "target_environment",
        "duration_seconds",
    ] {
        assert!(soak.contains(&format!("name=\"{name}\"")), "{name}");
    }

    for (id, names) in [
        (
            "branch-restriction-fields",
            vec!["branch_pattern", "target_environment"],
        ),
        (
            "approval-fields",
            vec!["target_environment", "required_approvals"],
        ),
        (
            "supersede-pending-fields",
            vec!["target_environment", "same_branch_only"],
        ),
    ] {
        let hidden = section(&html, id);
        assert!(
            element(hidden, &format!("id=\"{id}\"")).contains("class=\"hidden\""),
            "{id}"
        );
        let hidden_controls = controls(hidden);
        assert_eq!(hidden_controls.len(), names.len(), "{hidden}");
        for control in hidden_controls {
            assert!(control.contains(" disabled"), "{id}: {control}");
        }
        for name in names {
            assert!(hidden.contains(&format!("name=\"{name}\"")), "{id} {name}");
        }
    }
}

#[tokio::test]
async fn creating_a_soak_time_policy_sends_its_settings() {
    let app = app(true).await;
    let (status, location) = app
        .post(
            &format!("{PROJECT}/settings/policies"),
            "csrf_token=test-csrf&name=soak&policy_type=soak_time&source_environment=staging&target_environment=prod&duration_seconds=1800",
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.unwrap(), format!("{PROJECT}/settings/policies"));
    let created = app.created.lock().unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].name, "soak");
    assert!(matches!(
        &created[0].config,
        PolicyConfig::SoakTime { source_environment, target_environment, duration_seconds: 1800 }
            if source_environment == "staging" && target_environment == "prod"
    ));
}

#[tokio::test]
async fn creating_a_supersede_pending_policy_keeps_same_branch_only() {
    let app = app(true).await;
    app.post(
        &format!("{PROJECT}/settings/policies"),
        "csrf_token=test-csrf&name=collapse&policy_type=supersede_pending&target_environment=prod&same_branch_only=1",
    )
    .await;
    let created = app.created.lock().unwrap();
    assert!(matches!(
        &created[0].config,
        PolicyConfig::SupersedePending { target_environment, same_branch_only: true } if target_environment == "prod"
    ));
}

#[tokio::test]
async fn a_member_sees_policies_without_forms_or_scripts() {
    let app = app(false).await;
    let (status, html) = app.get(&format!("{PROJECT}/settings/policies")).await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("staging-soak-30m"));
    assert!(!main_element(&html).contains("<form"));
    assert!(!html.contains("policy-form.js"));
}

#[tokio::test]
async fn the_policies_page_without_policies_invites_an_admin_to_create_one() {
    let app = app_with(true, vec![], vec![]).await;
    let (_, html) = app.get(&format!("{PROJECT}/settings/policies")).await;
    assert!(html.contains("No deployment policies configured."));
    assert!(html.contains("Create one below to gate deployments"));
}

#[tokio::test]
async fn the_policy_edit_page_selects_the_current_settings() {
    let app = app(true).await;
    let (status, html) = app
        .get(&format!("{PROJECT}/settings/policies/{SOAK_ID}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    let main = main_element(&html);
    assert!(main.contains(&format!(
        "<form method=\"post\" action=\"{PROJECT}/settings/policies/{SOAK_ID}\" class=\"space-y-4\">"
    )));
    assert!(main.contains("<input type=\"hidden\" name=\"csrf_token\" value=\"test-csrf\">"));
    assert!(main.contains("<input type=\"hidden\" name=\"policy_type\" value=\"soak_time\">"));
    let source = &main[main.find("name=\"source_environment\"").unwrap()..];
    let source = &source[..source.find("</select>").unwrap()];
    assert!(
        source.contains("<option value=\"staging\" selected>"),
        "{source}"
    );
    let target = &main[main.find("name=\"target_environment\"").unwrap()..];
    let target = &target[..target.find("</select>").unwrap()];
    assert!(
        target.contains("<option value=\"prod\" selected>"),
        "{target}"
    );
    assert!(main.contains("name=\"duration_seconds\" min=\"1\" value=\"5400\""));
    assert!(main.contains(&format!("href=\"{PROJECT}/settings/policies\"")));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn the_policy_edit_page_renders_each_type_as_itself() {
    let app = app(true).await;
    let (_, html) = app
        .get(&format!("{PROJECT}/settings/policies/{BRANCH_ID}"))
        .await;
    assert!(html.contains("name=\"branch_pattern\" value=\"release/&lt;v&gt;\""));
    assert!(html.contains("Branch Restriction"));

    let (_, html) = app
        .get(&format!("{PROJECT}/settings/policies/{SUPERSEDE_ID}"))
        .await;
    assert!(html.contains("name=\"same_branch_only\" value=\"1\" checked"));
    assert!(html.contains("never interrupted"));

    let (_, html) = app
        .get(&format!("{PROJECT}/settings/policies/{APPROVAL_ID}"))
        .await;
    assert_clean(&html);
    assert!(html.contains("name=\"policy_type\" value=\"approval\""));
    assert!(html.contains("name=\"required_approvals\" min=\"1\" value=\"2\""));
    assert!(html.contains("name=\"target_environment\""));
}

#[tokio::test]
async fn saving_an_approval_policy_updates_its_approvals() {
    let app = app(true).await;
    let (status, location) = app
        .post(
            &format!("{PROJECT}/settings/policies/{APPROVAL_ID}"),
            "csrf_token=test-csrf&policy_type=approval&target_environment=prod&required_approvals=3",
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.unwrap(), format!("{PROJECT}/settings/policies"));
    let updated = app.updated.lock().unwrap();
    assert_eq!(updated[0].0, "prod-approval");
    assert!(matches!(
        &updated[0].1.config,
        Some(PolicyConfig::Approval { target_environment, required_approvals: 3 }) if target_environment == "prod"
    ));
    assert_eq!(updated[0].1.enabled, None);
}

#[tokio::test]
async fn saving_a_soak_time_policy_sends_the_edited_settings() {
    let app = app(true).await;
    app.post(
        &format!("{PROJECT}/settings/policies/{SOAK_ID}"),
        "csrf_token=test-csrf&policy_type=soak_time&source_environment=dev&target_environment=staging&duration_seconds=60",
    )
    .await;
    let updated = app.updated.lock().unwrap();
    assert_eq!(updated[0].0, "staging-soak-30m");
    assert!(matches!(
        &updated[0].1.config,
        Some(PolicyConfig::SoakTime { source_environment, target_environment, duration_seconds: 60 })
            if source_environment == "dev" && target_environment == "staging"
    ));
}

#[tokio::test]
async fn toggling_a_policy_sends_only_enabled() {
    let app = app(true).await;
    app.post(
        &format!("{PROJECT}/settings/policies/{BRANCH_ID}/toggle"),
        "csrf_token=test-csrf&enabled=true",
    )
    .await;
    let updated = app.updated.lock().unwrap();
    assert_eq!(updated[0].0, "main-only");
    assert_eq!(updated[0].1.enabled, Some(true));
    assert!(updated[0].1.config.is_none());
}

#[tokio::test]
async fn the_pipelines_page_hands_each_pipeline_to_the_builder() {
    let app = app(true).await;
    let (status, html) = app.get(&format!("{PROJECT}/settings/pipelines")).await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    let main = main_element(&html);
    let base = format!("{PROJECT}/settings/pipelines/p1");
    assert!(main.contains(&format!("<form method=\"post\" action=\"{base}/toggle\">")));
    assert!(main.contains(&format!(
        "<form method=\"post\" action=\"{base}/delete\" data-confirm=\"Delete pipeline &quot;standard&quot;?\">"
    )));
    assert!(main.contains(&format!(
        "<form method=\"post\" action=\"{base}/update\" class=\"space-y-3\">"
    )));
    assert!(main.contains(&format!(
        "<form method=\"post\" action=\"{PROJECT}/settings/pipelines\" class=\"space-y-4\">"
    )));
    assert!(main.contains("<input type=\"hidden\" name=\"_csrf\" value=\"test-csrf\">"));
    assert!(!main.contains("name=\"csrf_token\""));
    assert!(main.contains("<pipeline-builder data-target=\"edit-standard\"></pipeline-builder>"));
    assert!(main.contains("<textarea id=\"edit-standard\" name=\"stages_json\" hidden>[{&quot;id&quot;:&quot;dev&quot;"));
    assert!(!main.contains("</textarea><b>"));
    assert!(main.contains("dev&lt;/textarea&gt;&lt;b&gt;"));
    assert!(main.contains("<pipeline-builder data-target=\"pipeline-stages\"></pipeline-builder>"));
    assert!(
        main.contains("<textarea id=\"pipeline-stages\" name=\"stages_json\" hidden></textarea>")
    );
    assert!(main.contains("2 stages"));
    assert!(main.contains("no stages"));
    assert!(html.contains("<script src=\"/static/js/pipeline-builder.js?v="));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn a_member_sees_pipelines_read_only() {
    let app = app(false).await;
    let (status, html) = app.get(&format!("{PROJECT}/settings/pipelines")).await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    let main = main_element(&html);
    assert!(!main.contains("<form"));
    assert!(main.contains(
        "<pipeline-builder data-target=\"view-standard\" data-readonly=\"true\"></pipeline-builder>"
    ));
    assert!(main.contains("<textarea id=\"view-standard\" hidden>"));
    assert!(main.contains("No stages configured yet."));
    assert!(!main.contains("view-empty"));
    assert!(html.contains("pipeline-builder.js?v="));
}

#[tokio::test]
async fn the_pipelines_page_without_pipelines_invites_an_admin_to_create_one() {
    let app = app_with(true, vec![], vec![]).await;
    let (_, html) = app.get(&format!("{PROJECT}/settings/pipelines")).await;
    assert!(html.contains("No release pipelines configured."));
    assert!(html.contains("Create one below to define a multi-stage deployment DAG."));
}

#[tokio::test]
async fn saving_a_pipeline_posts_its_stages_with_the_underscore_csrf_field() {
    let app = app(true).await;
    let stages = urlencoding::encode(
        r#"[{"id":"dev","depends_on":[],"config":{"Deploy":{"environment":"dev"}}}]"#,
    );
    let (status, location) = app
        .post(
            &format!("{PROJECT}/settings/pipelines/p1/update"),
            &format!("_csrf=test-csrf&stages_json={stages}"),
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.unwrap(), format!("{PROJECT}/settings/pipelines"));
    let updates = app.pipeline_updates.lock().unwrap();
    assert_eq!(updates[0].0, "standard");
    assert_eq!(updates[0].1.stages.as_ref().unwrap().len(), 1);
    assert_eq!(updates[0].1.enabled, None);
}
