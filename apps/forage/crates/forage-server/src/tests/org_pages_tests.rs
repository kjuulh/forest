use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::compute::mock_scheduler::InMemoryComputeScheduler;
use forage_core::compute::{ComputeResourceSpec, ComputeScheduler, ResourceKind};
use forage_core::platform::{
    AllowedDomain, OrgMember, OrgPolicyRule, OrgRuleSet, PolicyConfig, Project, ProjectMetadata,
    ProjectSelector, Visibility,
};
use tower::ServiceExt;

use forage_core::session::InMemorySessionStore;

use crate::state::AppState;
use crate::test_support::*;

struct App {
    router: axum::Router,
    cookie: String,
}

async fn app_with(state: AppState, sessions: Arc<InMemorySessionStore>, admin: bool) -> App {
    let cookie = if admin {
        create_test_session(&sessions).await
    } else {
        create_test_session_member(&sessions).await
    };
    App {
        router: crate::build_router(state),
        cookie,
    }
}

async fn app(platform: MockPlatformClient, admin: bool) -> App {
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    app_with(state, sessions, admin).await
}

async fn oauth_app() -> App {
    let (state, sessions) = test_state_with_oauth_apps(Arc::new(MockOAuthAppsClient::new()));
    app_with(state, sessions, true).await
}

impl App {
    async fn get(&self, uri: &str) -> (StatusCode, String) {
        self.send(
            Request::builder()
                .uri(uri)
                .header("cookie", &self.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn post(&self, uri: &str, form: &str) -> (StatusCode, String) {
        self.send(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("cookie", &self.cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form.to_string()))
                .unwrap(),
        )
        .await
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, String) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let location = response
            .headers()
            .get("location")
            .map(|l| l.to_str().unwrap().to_string());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        (status, location.unwrap_or(body))
    }
}

fn main_element(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    html[start..end].to_string()
}

fn masked_main(html: &str) -> String {
    let main = main_element(html);
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&main, "?v=HASH");
    let main = regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .unwrap()
        .replace_all(&main, "ID");
    regex::Regex::new(r">[^<>]*\bago<")
        .unwrap()
        .replace_all(&main, ">AGO<")
        .into_owned()
}

fn assert_clean(html: &str) {
    let inline = regex::Regex::new(r"<script(?: [^>]*)?>([^<]+)</script>").unwrap();
    for tag in inline.find_iter(html) {
        assert!(
            tag.as_str()
                .starts_with(r#"<script type="application/json""#),
            "inline script: {}",
            tag.as_str()
        );
    }
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    assert!(!html.contains("onsubmit"), "no inline handlers: {html}");
    assert!(!html.contains("onclick"), "no inline handlers: {html}");
    assert!(!html.contains("style="), "no inline styles: {html}");
    let main = main_element(html);
    assert!(!main.contains(">none<"), "no none leak: {main}");
    assert!(!main.contains("value=\"none\""), "no none leak: {main}");
    assert!(!main.contains("/none"), "no none leak: {main}");
}

fn forms(html: &str) -> Vec<String> {
    let re = regex::Regex::new(r"(?s)<form[^>]*>.*?</form>").unwrap();
    re.find_iter(html).map(|m| m.as_str().to_string()).collect()
}

fn form_with_action<'a>(forms: &'a [String], action: &str) -> &'a str {
    let needle = format!(r#"action="{action}""#);
    forms
        .iter()
        .find(|f| f.contains(&needle))
        .unwrap_or_else(|| panic!("no form posting to {action}: {forms:#?}"))
}

fn members_platform() -> MockPlatformClient {
    MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_members_result: Some(Ok(vec![
            OrgMember {
                user_id: "user-123".into(),
                username: "testuser".into(),
                role: "owner".into(),
                joined_at: Some("2026-01-01T00:00:00Z".into()),
            },
            OrgMember {
                user_id: "user-456".into(),
                username: "alice".into(),
                role: "admin".into(),
                joined_at: None,
            },
        ])),
        ..Default::default()
    })
}

#[tokio::test]
async fn members_lets_an_admin_add_change_and_remove_everyone_but_the_owner() {
    let app = app(members_platform(), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/members").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("<title>Members - testorg - Forest</title>"));
    assert!(html.contains(r#"<a href="/orgs/testorg/settings/members" aria-current="page""#));
    let forms = forms(&html);
    let add = form_with_action(&forms, "/orgs/testorg/settings/members");
    assert!(add.contains(r#"method="POST""#));
    assert!(add.contains(r#"<input type="hidden" name="_csrf" value="test-csrf">"#));
    assert!(add.contains(r#"name="username" required"#));
    assert!(add.contains(r#"<option value="member">Member</option>"#));
    assert!(add.contains(r#"<option value="admin">Admin</option>"#));
    let role = form_with_action(&forms, "/orgs/testorg/settings/members/user-456/role");
    assert!(role.contains(r#"name="_csrf" value="test-csrf""#));
    assert!(role.contains(r#"<option value="admin" selected>Admin</option>"#));
    assert!(role.contains(r#"<option value="member">Member</option>"#));
    let remove = form_with_action(&forms, "/orgs/testorg/settings/members/user-456/remove");
    assert!(remove.contains(r#"name="_csrf" value="test-csrf""#));
    assert!(
        !html.contains("/members/user-123/"),
        "the owner is not editable"
    );
    assert!(html.contains(r#"<time datetime="2026-01-01T00:00:00Z""#));
    assert!(html.contains("—"), "an unknown join date is a dash");
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn members_shows_a_plain_list_to_a_member() {
    let app = app(members_platform(), false).await;
    let (status, html) = app.get("/orgs/testorg/settings/members").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("alice"));
    assert!(forms(&main_element(&html)).is_empty(), "{html}");
    assert!(!html.contains("Actions"));
}

#[tokio::test]
async fn a_role_change_posts_its_fields_and_returns_to_members() {
    let app = app(members_platform(), true).await;
    let (status, location) = app
        .post(
            "/orgs/testorg/settings/members/user-456/role",
            "_csrf=test-csrf&role=member",
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/orgs/testorg/settings/members");
}

fn access_platform() -> MockPlatformClient {
    MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_allowed_domains_result: Some(Ok(vec![
            AllowedDomain {
                domain: "understory.io".into(),
                policy: "invite".into(),
                dns_verified: false,
                dns_verification_token: "tok-abc123".into(),
                created_at: None,
            },
            AllowedDomain {
                domain: "verified.example".into(),
                policy: "invite".into(),
                dns_verified: true,
                dns_verification_token: "tok-done".into(),
                created_at: Some("2026-01-01T00:00:00Z".into()),
            },
        ])),
        ..Default::default()
    })
}

#[tokio::test]
async fn access_gives_an_admin_add_verify_and_remove_forms() {
    let app = app(access_platform(), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/access").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    let forms = forms(&html);
    let add = form_with_action(&forms, "/orgs/testorg/settings/access");
    assert!(add.contains(r#"name="_csrf" value="test-csrf""#));
    assert!(add.contains(r#"name="domain""#));
    let verify = form_with_action(&forms, "/orgs/testorg/settings/access/verify");
    assert!(verify.contains(r#"<input type="hidden" name="domain" value="understory.io">"#));
    assert!(verify.contains(r#"name="_csrf" value="test-csrf""#));
    let removes: Vec<_> = forms
        .iter()
        .filter(|f| f.contains(r#"action="/orgs/testorg/settings/access/remove""#))
        .collect();
    assert_eq!(removes.len(), 2);
    assert!(removes[1].contains(r#"value="verified.example""#));
    assert!(
        !html.contains("tok-done"),
        "a verified domain hides its token"
    );
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn access_is_read_only_for_a_member() {
    let app = app(access_platform(), false).await;
    let (status, html) = app.get("/orgs/testorg/settings/access").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("understory.io"));
    assert!(forms(&main_element(&html)).is_empty());
    assert!(!html.contains("tok-abc123"));
}

#[tokio::test]
async fn access_without_domains_says_so() {
    let app = app(MockPlatformClient::new(), true).await;
    let (_, html) = app.get("/orgs/testorg/settings/access").await;
    assert!(html.contains("No domains configured."));
}

#[tokio::test]
async fn org_visibility_renders_its_form_in_the_settings_layout() {
    let app = app(MockPlatformClient::new(), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/visibility").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains(r#"<a href="/orgs/testorg/settings/visibility" aria-current="page""#));
    let forms = forms(&html);
    let form = form_with_action(&forms, "/orgs/testorg/settings/visibility");
    assert!(form.contains(r#"name="_csrf" value="test-csrf""#));
    assert!(form.contains(r#"name="visibility" value="private" checked"#));
    assert!(form.contains(r#"name="visibility" value="public" class"#));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn project_visibility_uses_the_project_settings_layout() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_visibility_result: Some(Ok(Visibility {
            organisation: "public".into(),
            project: Some("private".into()),
            can_change: true,
        })),
        ..Default::default()
    });
    let app = app(platform, true).await;
    let (status, html) = app
        .get("/orgs/testorg/projects/widget/settings/visibility")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("<title>Visibility - widget - testorg - Forest</title>"));
    assert!(html.contains(
        r#"<a href="/orgs/testorg/projects/widget/settings/visibility" aria-current="page""#
    ));
    assert!(html.contains("Who outside testorg can read this component"));
    assert!(html.contains("through the organisation"));
    let forms = forms(&html);
    let form = form_with_action(&forms, "/orgs/testorg/projects/widget/settings/visibility");
    assert!(form.contains(r#"name="visibility" value="private" checked"#));
    assert!(form.contains("Visibility of testorg/widget"));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn usage_shows_the_project_count_with_no_section_marked() {
    let app = app(MockPlatformClient::new(), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/usage").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("<title>Usage - testorg - Forest</title>"));
    assert!(html.contains(r#"<p class="text-2xl font-bold">1</p>"#));
    let main = main_element(&html);
    assert!(
        !main.contains("aria-current"),
        "usage is not a sidebar section"
    );
    insta::assert_snapshot!(masked_main(&html));
}

async fn compute_app() -> App {
    let scheduler = Arc::new(InMemoryComputeScheduler::new());
    let labels = HashMap::from([
        ("project".to_string(), "my-api".to_string()),
        ("destination".to_string(), "prod-eu".to_string()),
        ("environment".to_string(), "prod".to_string()),
        ("region".to_string(), "eu-west-1".to_string()),
    ]);
    scheduler
        .apply_resources(
            "apply-1",
            "testorg",
            vec![
                ComputeResourceSpec {
                    name: "web".into(),
                    kind: ResourceKind::ContainerService,
                    image: Some("registry/web:1".into()),
                    replicas: 2,
                    cpu: None,
                    memory: None,
                },
                ComputeResourceSpec {
                    name: "web-route".into(),
                    kind: ResourceKind::Route,
                    image: None,
                    replicas: 0,
                    cpu: None,
                    memory: None,
                },
            ],
            labels,
        )
        .await
        .unwrap();
    let (state, sessions) = test_state();
    let state = state.with_compute_scheduler(scheduler);
    app_with(state, sessions, true).await
}

#[tokio::test]
async fn compute_lists_instances_and_links_rollouts() {
    let app = compute_app().await;
    let (status, html) = app.get("/orgs/testorg/settings/compute").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains(r#"<a href="/orgs/testorg/projects/my-api""#));
    assert!(html.contains("registry/web:1"));
    assert!(html.contains("250m / 256Mi"));
    assert!(html.contains("2 resources"));
    let rollout =
        regex::Regex::new(r#"href="/orgs/testorg/settings/compute/rollouts/([0-9a-f-]{36})""#)
            .unwrap();
    assert!(rollout.is_match(&html), "{html}");
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn compute_without_a_scheduler_shows_the_empty_state() {
    let app = app(MockPlatformClient::new(), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/compute").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("No compute instances running"));
    assert!(html.contains(r#"href="/orgs/testorg/destinations""#));
    assert!(!html.contains("Recent Rollouts"));
}

#[tokio::test]
async fn rollout_detail_lists_resources_and_sorted_labels() {
    let app = compute_app().await;
    let (_, html) = app.get("/orgs/testorg/settings/compute").await;
    let id = regex::Regex::new(r#"/settings/compute/rollouts/([0-9a-f-]{36})""#)
        .unwrap()
        .captures(&html)
        .unwrap()[1]
        .to_string();
    let (status, html) = app
        .get(&format!("/orgs/testorg/settings/compute/rollouts/{id}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("<title>Rollout apply-1 - Forest</title>"));
    assert!(html.contains("Resources (2)"));
    assert!(html.contains("container_service"));
    let destination = html.find("destination=prod-eu").unwrap();
    let region = html.find("region=eu-west-1").unwrap();
    assert!(destination < region, "labels are sorted");
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn developers_lists_apps_without_their_secret() {
    let app = oauth_app().await;
    let (status, html) = app
        .post(
            "/orgs/testorg/settings/developers",
            "_csrf=test-csrf&name=My+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on&scope_openid=on",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("forest_oas_secret_"));
    let (status, html) = app.get("/orgs/testorg/settings/developers").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(!html.contains("forest_oas_secret_"));
    assert!(html.contains(r#"href="/orgs/testorg/settings/developers/app-1""#));
    assert!(html.contains(r#"href="/orgs/testorg/settings/developers/new""#));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn the_new_app_form_keeps_every_field_and_its_defaults() {
    let app = oauth_app().await;
    let (status, html) = app
        .get("/orgs/testorg/settings/developers/new?error=Name+taken")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("Name taken"));
    let forms = forms(&html);
    let form = form_with_action(&forms, "/orgs/testorg/settings/developers");
    for field in [
        r#"name="_csrf" value="test-csrf""#,
        r#"name="name""#,
        r#"name="description""#,
        r#"name="homepage_url""#,
        r#"name="redirect_uris""#,
        r#"name="scope_openid" class"#,
        r#"name="scope_profile" checked"#,
        r#"name="scope_email" class"#,
        r#"name="scope_components_publish" class"#,
        r#"name="scope_tokens_introspect" class"#,
        r#"name="scope_datagateway_apply" class"#,
        r#"name="scope_datagateway_read" class"#,
        r#"name="grant_authorization_code" checked"#,
        r#"name="grant_client_credentials" class"#,
    ] {
        assert!(form.contains(field), "{field}: {form}");
    }
    assert!(!form.contains("grant_device_code"));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn an_app_shows_its_secret_once_then_its_saved_settings() {
    let app = oauth_app().await;
    let (_, created) = app
        .post(
            "/orgs/testorg/settings/developers",
            "_csrf=test-csrf&name=My+App&redirect_uris=https%3A%2F%2Fa.example%2Fcb%0Ahttps%3A%2F%2Fb.example%2Fcb&scope_email=on&grant_client_credentials=on",
        )
        .await;
    assert_clean(&created);
    assert!(created.contains("Client secret created"));
    assert!(created.contains("forest_oas_secret_"));

    let (status, html) = app.get("/orgs/testorg/settings/developers/app-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(!html.contains("Client secret created"));
    assert!(!html.contains("forest_oas_secret_"));
    let forms = forms(&html);
    let form = form_with_action(&forms, "/orgs/testorg/settings/developers/app-1");
    assert!(form.contains(r#"name="name" type="text" required maxlength="100" value="My App""#));
    assert!(form.contains("https://a.example/cb\nhttps://b.example/cb</textarea>"));
    assert!(form.contains(r#"name="scope_email" checked"#));
    assert!(form.contains(r#"name="scope_profile" class"#));
    assert!(form.contains(r#"name="grant_client_credentials" checked"#));
    assert!(form.contains(r#"name="grant_authorization_code" class"#));
    assert!(form.contains(r#"name="grant_device_code" class"#));
    let rotate = form_with_action(&forms, "/orgs/testorg/settings/developers/app-1/rotate");
    assert!(rotate.contains(r#"name="_csrf" value="test-csrf""#));
    let delete = form_with_action(&forms, "/orgs/testorg/settings/developers/app-1/delete");
    assert!(delete.contains(
        r#"data-confirm="Delete this OAuth application? All issued tokens will stop working.""#
    ));
    insta::assert_snapshot!(masked_main(&html));

    let (_, rotated) = app
        .post(
            "/orgs/testorg/settings/developers/app-1/rotate",
            "_csrf=test-csrf",
        )
        .await;
    assert!(rotated.contains("forest_oas_rotated_app-1"));
    let (_, again) = app.get("/orgs/testorg/settings/developers/app-1").await;
    assert!(!again.contains("forest_oas_rotated_app-1"));
}

#[tokio::test]
async fn an_app_shows_an_update_refusal() {
    let app = oauth_app().await;
    app.post(
        "/orgs/testorg/settings/developers",
        "_csrf=test-csrf&name=My+App&redirect_uris=https%3A%2F%2Fa.example%2Fcb",
    )
    .await;
    let (status, html) = app
        .get("/orgs/testorg/settings/developers/app-1?error=Redirect+URI+must+be+https")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Redirect URI must be https"), "{html}");
}

fn rules_platform(name: &str) -> MockPlatformClient {
    let mut metadata = BTreeMap::new();
    metadata.insert("domain".into(), "retail".into());
    MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_org_rule_sets_result: Some(Ok(vec![OrgRuleSet {
            organisation: "testorg".into(),
            name: name.into(),
            enabled: true,
            selector: ProjectSelector {
                include_projects: vec!["shop".into()],
                exclude_projects: vec!["sandbox".into()],
                name_regex: Some("^sh".into()),
                metadata_match: metadata,
                tags: vec!["web".into()],
            },
            policies: vec![OrgPolicyRule {
                name: "prod-main-only".into(),
                enabled: true,
                config: PolicyConfig::BranchRestriction {
                    target_environment: "prod".into(),
                    branch_pattern: "^main$".into(),
                },
            }],
            triggers: vec![],
            release_pipelines: vec![],
            created_at: "2026-08-30T00:00:00Z".into(),
            updated_at: "2026-08-30T00:00:00Z".into(),
        }])),
        list_projects_result: Some(Ok(vec!["shop".into()])),
        get_project_result: Some(Ok(Some(Project {
            organisation: "testorg".into(),
            project: "shop".into(),
            metadata: ProjectMetadata {
                domain: "</script><script>alert(1)</script>".into(),
                ..Default::default()
            },
            ..Default::default()
        }))),
        ..Default::default()
    })
}

#[tokio::test]
async fn org_rules_renders_rules_the_create_form_and_its_scripts_as_files() {
    let app = app(rules_platform("retail-prod"), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/rules").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("<title>Rules - testorg - Forest</title>"));
    assert!(html.contains(r#"<a href="/orgs/testorg/settings/rules" aria-current="page""#));
    let forms = forms(&html);
    let delete = form_with_action(&forms, "/orgs/testorg/settings/rules/retail-prod/delete");
    assert!(delete.contains(r#"data-confirm="Delete organisation rule set retail-prod?""#));
    assert!(delete.contains(r#"name="csrf_token" value="test-csrf""#));
    let create = form_with_action(&forms, "/orgs/testorg/settings/rules");
    for field in [
        r#"name="csrf_token" value="test-csrf""#,
        r#"name="name" required"#,
        r#"name="enabled" checked"#,
        r#"name="include_projects""#,
        r#"name="exclude_projects""#,
        r#"name="name_regex""#,
        r#"name="tags""#,
        r#"name="metadata_match""#,
        r#"name="policies_json""#,
        r#"name="triggers_json""#,
        r#"name="pipelines_json""#,
    ] {
        assert!(create.contains(field), "{field}: {create}");
    }
    for hidden in [
        "org-policy-source-env",
        "org-policy-soak-target-env",
        "org-policy-duration",
        "org-policy-approval-target-env",
        "org-policy-approvals",
        "org-policy-supersede-target-env",
        "org-policy-supersede-same-branch",
    ] {
        let re = regex::Regex::new(&format!(r#"<input id="{hidden}"[^>]* disabled"#)).unwrap();
        assert!(re.is_match(create), "{hidden} starts disabled: {create}");
    }
    let re = regex::Regex::new(r#"<input id="org-policy-branch-pattern"[^>]*>"#).unwrap();
    assert!(!re.find(create).unwrap().as_str().contains("disabled"));
    assert!(
        create
            .contains(r#"<pipeline-builder data-target="org-pipeline-stages"></pipeline-builder>"#)
    );
    assert!(html.contains("Name regex:</span> <code>^sh</code>"));
    assert!(html.contains("Include:</span> shop"));
    assert!(html.contains(r#"data-rule-preview="retail-prod""#));
    let script =
        regex::Regex::new(r#"<script src="/static/js/org-rules\.js\?v=[0-9a-z]+" defer></script>"#)
            .unwrap();
    assert!(script.is_match(&html), "{html}");
    assert!(html.contains(r#"<script src="/static/js/pipeline-builder.js?v="#));
    insta::assert_snapshot!(masked_main(&html));
}

#[tokio::test]
async fn org_rules_data_blocks_keep_script_closers_escaped() {
    let app = app(rules_platform("</script><script>alert(2)</script>"), true).await;
    let (status, html) = app.get("/orgs/testorg/settings/rules").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("<script>alert"), "{html}");
    for id in ["org-rule-projects-data", "org-rule-sets-data"] {
        let open = format!(r#"<script type="application/json" id="{id}">"#);
        let start = html.find(&open).unwrap_or_else(|| panic!("{id}: {html}")) + open.len();
        let block = &html[start..start + html[start..].find("</script>").unwrap()];
        let value: serde_json::Value = serde_json::from_str(block).expect("valid JSON");
        assert!(value.is_array(), "{block}");
        assert!(block.contains("\\u003c/script\\u003e"), "{id}: {block}");
    }
}

#[tokio::test]
async fn org_rules_hides_admin_forms_from_a_member() {
    let app = app(rules_platform("retail-prod"), false).await;
    let (status, html) = app.get("/orgs/testorg/settings/rules").await;
    assert_eq!(status, StatusCode::OK);
    assert_clean(&html);
    assert!(html.contains("retail-prod"));
    assert!(forms(&main_element(&html)).is_empty());
    assert!(!html.contains("Create rule set"));
}
