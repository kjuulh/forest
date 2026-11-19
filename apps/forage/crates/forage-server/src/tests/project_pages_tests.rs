use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    Artifact, ArtifactContext, ArtifactDestination, JoinOffer, Project, ProjectMetadata,
};
use forage_core::registry::{ComponentDetail, ComponentSummary, ComponentVersionInfo, ToolShape};
use tower::ServiceExt;

use crate::build_router;
use crate::state::AppState;
use crate::test_support::*;

async fn get(state: AppState, cookie: &str, uri: &str) -> (StatusCode, String) {
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
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

fn main_snapshot(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&html[start..end], "?v=HASH")
        .into_owned();
    regex::Regex::new(r"(<time [^>]*>)[^<]*(</time>)")
        .unwrap()
        .replace_all(&main, "${1}AGO${2}")
        .into_owned()
}

fn assert_no_inline_script(html: &str) {
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    assert!(
        !html.contains("<script defer>"),
        "no inline scripts: {html}"
    );
    assert!(!html.contains(" oninput="), "no inline handlers: {html}");
    assert!(!html.contains(" onclick="), "no inline handlers: {html}");
}

fn artifact(slug: &str, title: &str, created_at: &str, envs: &[&str]) -> Artifact {
    Artifact {
        artifact_id: format!("id-{slug}"),
        slug: slug.into(),
        context: ArtifactContext {
            title: title.into(),
            description: None,
            web: None,
            pr: None,
        },
        source: None,
        git_ref: None,
        destinations: envs
            .iter()
            .map(|env| ArtifactDestination {
                name: format!("{env}-dest"),
                environment: (*env).into(),
                type_organisation: None,
                type_name: None,
                type_version: None,
                status: None,
            })
            .collect(),
        created_at: created_at.into(),
    }
}

fn join_offer() -> JoinOffer {
    JoinOffer {
        organisation_id: "org-42".into(),
        organisation_name: "Acme <Corp>".into(),
        matched_domain: "acme.test".into(),
    }
}

#[tokio::test]
async fn dashboard_lists_orgs_recent_releases_and_join_offers() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec!["my-api".into()])),
        list_artifacts_result: Some(Ok(vec![
            artifact("older", "Older release", "2026-01-01T00:00:00Z", &[]),
            artifact(
                "newer",
                "Newer release",
                "2026-02-01T00:00:00Z",
                &["dev", "prod", "dev"],
            ),
        ])),
        list_join_offers_result: Some(Ok(vec![join_offer()])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, &cookie, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<title>Dashboard - Forest</title>"));
    assert!(html.contains(r#"<form method="POST" action="/orgs" class="flex gap-2 mt-3">"#));
    assert!(html.contains(r#"<input type="hidden" name="_csrf" value="test-csrf">"#));
    assert!(html.contains(r#"name="name" placeholder="new-org-name""#));
    assert!(html.contains(r#"action="/join-offers/org-42/accept""#));
    assert!(html.contains("Acme &lt;Corp&gt;"));
    assert!(html.contains(r#"href="/orgs/testorg/projects/my-api/releases/newer""#));
    assert!(
        html.find("Newer release").unwrap() < html.find("Older release").unwrap(),
        "most recent first"
    );
    assert_eq!(
        html.matches(">dev</span>").count(),
        1,
        "environments are de-duplicated"
    );
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects" class="whitespace-nowrap"#),
        "org tabs for the first org"
    );
    assert!(
        html.contains("Select project"),
        "the first org's project switcher"
    );
    assert!(!html.contains(">none<"));
    assert_no_inline_script(&html);
    insta::assert_snapshot!(main_snapshot(&html));
}

#[tokio::test]
async fn dashboard_without_orgs_is_the_onboarding_page() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_orgs_result: Some(Ok(vec![])),
        list_join_offers_result: Some(Ok(vec![join_offer()])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session_no_orgs(&sessions).await;
    let (status, html) = get(state, &cookie, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<title>Get Started - Forest</title>"));
    assert!(html.contains("Welcome to Forest"));
    assert!(
        html.contains(r#"<form method="POST" action="/orgs" class="mt-8 text-left space-y-4">"#)
    );
    assert!(html.contains(r#"<input type="hidden" name="_csrf" value="test-csrf">"#));
    assert!(html.contains(r#"action="/join-offers/org-42/accept""#));
    assert!(
        html.contains(
            "cargo install forest\n\n<span class=\"text-gray-500\"># Create an organisation</span>"
        ),
        "the CLI snippet keeps its blank lines: {html}"
    );
    assert!(!html.contains("bg-red-50"), "no error without one");
    assert_no_inline_script(&html);
    insta::assert_snapshot!(main_snapshot(&html));
}

#[tokio::test]
async fn creating_an_org_with_an_invalid_name_shows_onboarding_with_the_error() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_orgs_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session_no_orgs(&sessions).await;
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&name=Bad%20Name"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains(
            "Invalid organisation name. Use lowercase letters, numbers, and hyphens only."
        )
    );
    assert!(html.contains("Welcome to Forest"));
}

async fn projects_page(projects: Vec<String>) -> String {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(projects)),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, &cookie, "/orgs/testorg/projects").await;
    assert_eq!(status, StatusCode::OK);
    html
}

#[tokio::test]
async fn projects_page_filters_long_lists_with_a_script_file() {
    let html = projects_page((1..=6).map(|i| format!("service-{i}")).collect()).await;
    assert!(html.contains("<title>testorg - Projects - Forest</title>"));
    assert!(html.contains(r#"id="project-filter""#));
    assert!(html.contains("data-project-filter"));
    assert!(html.contains(r#"<script src="/static/js/project-filter.js?v="#));
    assert!(html.contains(r#"href="/orgs/testorg/projects/service-6""#));
    assert!(html.contains(r#"data-name="service-6""#));
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects" aria-current="page""#),
        "the Projects tab is current"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!(main_snapshot(&html));
}

#[tokio::test]
async fn projects_page_with_few_projects_has_no_filter() {
    let html = projects_page(vec!["my-api".into()]).await;
    assert!(html.contains(r#"href="/orgs/testorg/projects/my-api""#));
    assert!(!html.contains("project-filter"));
}

#[tokio::test]
async fn projects_page_without_projects_explains_how_to_create_one() {
    let html = projects_page(vec![]).await;
    assert!(html.contains("No projects yet"));
    assert!(html.contains("forest init my-project\nforest release create --env dev</pre>"));
    insta::assert_snapshot!(main_snapshot(&html));
}

fn summary(shape: ToolShape, latest_version: &str) -> ComponentSummary {
    ComponentSummary {
        organisation: "testorg".into(),
        name: "my-api".into(),
        latest_version: latest_version.into(),
        kind: "binary".into(),
        description: "From the registry".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-02-01T00:00:00Z".into(),
        version_count: 4,
        contracts: vec!["http".into()],
        visibility: "public".into(),
        shape,
        tool: None,
        methods: vec![],
        upstream_host: String::new(),
    }
}

fn version(v: &str) -> ComponentVersionInfo {
    ComponentVersionInfo {
        version: v.into(),
        protocol_version: "2".into(),
        kind: "binary".into(),
        platforms: vec!["linux-amd64".into(), "darwin-arm64".into()],
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn detail(shape: ToolShape) -> ComponentDetail {
    ComponentDetail {
        summary: summary(shape, "1.3.0"),
        versions: ["1.3.0", "1.2.0", "1.1.0", "1.0.0"]
            .into_iter()
            .map(version)
            .collect(),
        readme: "# Registry readme".into(),
        manifest_json: String::new(),
        owners: vec![],
    }
}

fn project_info(readme: &str) -> Project {
    Project {
        organisation: "testorg".into(),
        project: "my-api".into(),
        readme: readme.into(),
        description: "Serves the API".into(),
        metadata: ProjectMetadata {
            git_url: "https://github.com/rawpotion/my-api".into(),
            homepage: "https://www.example.com/".into(),
            docs_url: String::new(),
            support_url: "https://support.example.com/help".into(),
            domain: "payments".into(),
            owner: "team-a".into(),
            tags: vec![],
        },
    }
}

async fn project_detail(
    platform: MockPlatformBehavior,
    registry: Option<MockRegistryBehavior>,
    member: bool,
) -> String {
    let platform = MockPlatformClient::with_behavior(platform);
    let (state, sessions) = match registry {
        Some(registry) => test_state_with_registry(
            MockForestClient::new(),
            platform,
            MockRegistryClient::with_behavior(registry),
        ),
        None => test_state_with(MockForestClient::new(), platform),
    };
    let cookie = if member {
        create_test_session_member(&sessions).await
    } else {
        create_test_session(&sessions).await
    };
    let (status, html) = get(state, &cookie, "/orgs/testorg/projects/my-api").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

#[tokio::test]
async fn project_detail_shows_timeline_readme_about_and_install_commands() {
    let html = project_detail(
        MockPlatformBehavior {
            list_artifacts_result: Some(Ok(vec![artifact(
                "r1",
                "Release",
                "2026-02-01T00:00:00Z",
                &["dev"],
            )])),
            list_projects_result: Some(Ok(vec!["my-api".into()])),
            get_project_result: Some(Ok(Some(project_info(
                "# Hello\n\n<script>alert(1)</script>\n\n```rust\nfn main() {}\n```\n",
            )))),
            ..Default::default()
        },
        Some(MockRegistryBehavior {
            get_component_detail_result: Some(Ok(detail(ToolShape::ToolBinary))),
            ..Default::default()
        }),
        false,
    )
    .await;
    assert!(html.contains("<title>my-api - testorg - Forest</title>"));
    assert!(html.contains(r#"<release-timeline org="testorg" project="my-api" csrf="test-csrf" username="testuser" role="owner" limit="3"></release-timeline>"#), "{html}");
    assert!(html.contains(r#"data-timeline-url="/api/orgs/testorg/projects/my-api/timeline""#));
    assert!(html.contains("/static/js/components/forage-components.js?v="));
    assert!(
        html.contains("<h1>Hello</h1>"),
        "the project readme wins over the registry one"
    );
    assert!(!html.contains("Registry readme"));
    assert!(
        !html.contains("alert(1)"),
        "readme html is sanitised: {html}"
    );
    assert!(html.contains(r#"<script src="/static/js/vendor/highlight.min.js" defer></script>"#));
    assert!(html.contains(r#"<script src="/static/js/readme-highlight.js?v="#));
    assert!(
        html.contains("Serves the API"),
        "the project description wins"
    );
    assert!(
        html.contains(">rawpotion/my-api</span>"),
        "links are prettified"
    );
    assert!(html.contains(">example.com</span>"));
    assert!(html.contains(r#"href="https://support.example.com/help""#));
    assert!(!html.contains("Documentation"));
    assert!(html.contains(">payments</dd>") && html.contains(">team-a</dd>"));
    assert!(html.contains("Install"));
    assert!(html.contains("forest global add testorg/my-api</pre>"));
    assert!(html.contains("forest global add testorg/my-api@1.3.0</pre>"));
    assert!(html.contains(r#"data-copy="install-cmd-pinned""#));
    assert!(html.contains(r#"href="/orgs/testorg/projects/my-api/settings/visibility""#));
    assert!(html.contains(r#"href="/components/testorg/my-api/1.3.0""#));
    assert!(
        !html.contains(r#"href="/components/testorg/my-api/1.0.0""#),
        "only three versions"
    );
    assert!(html.contains("+ 1 more"));
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects/my-api" aria-current="page""#),
        "the Overview tab is current"
    );
    assert!(!html.contains(">none<"));
    assert_no_inline_script(&html);
    insta::assert_snapshot!(main_snapshot(&html));
    let head = &html[..html.find("</head>").expect("a head")];
    assert!(
        head.contains("/static/css/vendor/highlight-github.min.css"),
        "the README's highlight styles load in <head>, before the page paints"
    );
}

#[tokio::test]
async fn project_detail_readme_cannot_break_out_of_its_block() {
    let html = project_detail(
        MockPlatformBehavior {
            list_artifacts_result: Some(Ok(vec![])),
            get_project_result: Some(Ok(Some(project_info(
                "<img src=x onerror=alert(1)> </div><style>body{}</style>",
            )))),
            ..Default::default()
        },
        None,
        false,
    )
    .await;
    assert!(!html.contains("onerror"), "{html}");
    assert!(!html.contains("<style>"), "{html}");
}

#[tokio::test]
async fn project_detail_for_a_component_offers_the_add_command_to_members() {
    let html = project_detail(
        MockPlatformBehavior {
            list_artifacts_result: Some(Ok(vec![])),
            ..Default::default()
        },
        Some(MockRegistryBehavior {
            get_component_detail_result: Some(Ok(detail(ToolShape::Component))),
            ..Default::default()
        }),
        true,
    )
    .await;
    assert!(html.contains("forest components add testorg/my-api@1.3.0</pre>"));
    assert!(!html.contains("forest global add"));
    assert!(
        !html.contains("release-timeline"),
        "no releases, no timeline"
    );
    assert!(!html.contains("forage-components.js"));
    assert!(
        html.contains("From the registry"),
        "the registry description is the fallback"
    );
    assert!(html.contains("<h1>Registry readme</h1>"));
    assert!(
        !html.contains(">About<"),
        "no project metadata, no About block"
    );
}

#[tokio::test]
async fn project_detail_with_nothing_published_shows_how_to_start() {
    let html = project_detail(
        MockPlatformBehavior {
            list_artifacts_result: Some(Ok(vec![])),
            ..Default::default()
        },
        None,
        false,
    )
    .await;
    assert!(html.contains("Publish your first version"));
    assert!(html.contains(r#"data-copy="get-started-cmd""#));
    assert!(!html.contains("<aside"));
    assert!(!html.contains("release-timeline"));
    assert!(!html.contains("highlight.min.js"));
    assert_no_inline_script(&html);
    insta::assert_snapshot!(main_snapshot(&html));
}

async fn project_components(versions: Vec<ComponentVersionInfo>) -> String {
    let (state, sessions) = test_state_with_registry(
        MockForestClient::new(),
        MockPlatformClient::new(),
        MockRegistryClient::with_behavior(MockRegistryBehavior {
            list_component_versions_result: Some(Ok(versions)),
            ..Default::default()
        }),
    );
    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, &cookie, "/orgs/testorg/projects/my-api/components").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    html
}

#[tokio::test]
async fn project_components_lists_versions_newest_first() {
    let mut old = version("0.9.0");
    old.protocol_version = String::new();
    let html = project_components(vec![version("1.0.0"), old]).await;
    assert!(html.contains("<title>Components - my-api - testorg - Forest</title>"));
    assert!(html.contains(r#"href="/components/testorg/my-api/1.0.0""#));
    assert_eq!(html.matches(">latest</span>").count(), 1);
    assert_eq!(
        html.matches("protocol ").count(),
        1,
        "an unset protocol is not shown"
    );
    assert!(html.contains(">linux-amd64</span>"));
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects/my-api/components" aria-current="page""#)
    );
    insta::assert_snapshot!(main_snapshot(&html));
}

#[tokio::test]
async fn project_components_without_versions_says_so() {
    let html = project_components(vec![]).await;
    assert!(html.contains("No components published yet."));
    assert!(html.contains("Publish from this project directory: forest publish"));
}

#[tokio::test]
async fn project_detail_with_only_tags_shows_no_empty_about_heading() {
    let mut info = project_info("");
    info.metadata = ProjectMetadata {
        tags: vec!["internal".into()],
        ..Default::default()
    };
    let html = project_detail(
        MockPlatformBehavior {
            list_artifacts_result: Some(Ok(vec![artifact(
                "r1",
                "Release",
                "2026-02-01T00:00:00Z",
                &[],
            )])),
            get_project_result: Some(Ok(Some(info))),
            ..Default::default()
        },
        None,
        false,
    )
    .await;
    assert!(html.contains("<aside"));
    assert!(!html.contains("About"), "{html}");
}

#[tokio::test]
async fn projects_page_keeps_the_project_switcher_in_the_header() {
    let html = projects_page(vec!["api".into(), "web".into()]).await;
    assert!(html.contains(r#"data-fuzzy-label="Search projects""#), "{html}");
}
