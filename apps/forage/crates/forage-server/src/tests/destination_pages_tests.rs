use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    Destination, DestinationType, DestinationTypeInfo, Environment, MetadataFieldDef,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

fn environment(id: &str, name: &str, sort_order: i32) -> Environment {
    Environment {
        id: id.into(),
        organisation: "testorg".into(),
        name: name.into(),
        description: Some(format!("{name} environment")),
        sort_order,
        created_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn destination(name: &str, environment: &str, typed: bool) -> Destination {
    Destination {
        name: name.into(),
        environment: environment.into(),
        organisation: "testorg".into(),
        metadata: [
            ("region".to_string(), "eu-west-1".to_string()),
            ("account".to_string(), "1234".to_string()),
        ]
        .into(),
        sensitive_keys: vec!["api_token".into()],
        dest_type: typed.then(|| DestinationType {
            organisation: "forest".into(),
            name: "terraform".into(),
            version: 2,
        }),
    }
}

fn terraform_type(name: &str) -> DestinationTypeInfo {
    DestinationTypeInfo {
        organisation: "forest".into(),
        name: name.into(),
        version: 2,
        description: "Terraform workspace".into(),
        fields: vec![MetadataFieldDef {
            name: "region".into(),
            label: "Region".into(),
            description: String::new(),
            required: true,
            field_type: "text".into(),
            default_value: String::new(),
            sensitive: false,
        }],
    }
}

struct Setup {
    environments: Vec<Environment>,
    destinations: Vec<Destination>,
    types: Vec<DestinationTypeInfo>,
    admin: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            environments: vec![
                environment("env-dev", "dev", 0),
                environment("env-prod", "prod", 10),
            ],
            destinations: vec![
                destination("app-dev", "dev", false),
                destination("app prod&eu", "prod", true),
                destination("stray", "legacy", true),
            ],
            types: vec![terraform_type("terraform")],
            admin: true,
        }
    }
}

async fn app(setup: Setup) -> (axum::Router, String) {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_environments_result: Some(Ok(setup.environments)),
        list_destinations_result: Some(Ok(setup.destinations)),
        list_destination_types_result: Some(Ok(setup.types)),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = if setup.admin {
        create_test_session(&sessions).await
    } else {
        create_test_session_member(&sessions).await
    };
    (build_router(state), cookie)
}

async fn get(setup: Setup, uri: &str) -> String {
    let (app, cookie) = app(setup).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
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

async fn post(uri: &str, form: &str) -> (StatusCode, String) {
    let (app, cookie) = app(Setup::default()).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let location = response
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (response.status(), location)
}

fn main_element(html: &str) -> String {
    let start = html.find("<main").expect("a <main>");
    let end = html.find("</main>").expect("a </main>") + "</main>".len();
    regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&html[start..end], "?v=HASH")
        .into_owned()
}

fn assert_no_inline_behaviour(html: &str) {
    assert!(!html.contains("<script>"), "{html}");
    for attr in [" onclick=", " onchange=", " onsubmit=", " style="] {
        assert!(!html.contains(attr), "{attr} in {html}");
    }
    let inline = regex::Regex::new(r"<script(?: [^>]*)?>([^<]+)</script>").unwrap();
    for block in inline.captures_iter(html) {
        let tag = block.get(0).unwrap().as_str();
        assert!(
            tag.starts_with(r#"<script type="application/json""#),
            "inline script: {tag}"
        );
    }
}

const LIST: &str = "/orgs/testorg/destinations";

#[tokio::test]
async fn the_destinations_page_keeps_its_forms_and_field_names() {
    let html = get(Setup::default(), LIST).await;
    assert_no_inline_behaviour(&html);
    assert!(html.contains("/static/js/destinations.js?v="), "{html}");

    assert_eq!(
        html.matches(r#"action="/orgs/testorg/destinations/create""#)
            .count(),
        2
    );
    assert!(html.contains(r#"id="create-dest-form-1""#));
    assert!(html.contains(r#"id="create-dest-form-2""#));
    for field in [
        r#"name="_csrf" value="test-csrf""#,
        r#"name="environment" value="dev""#,
        r#"name="environment" value="prod""#,
        r#"name="type_organisation" id="type_organisation_1""#,
        r#"name="type_name" id="type_name_1""#,
        r#"name="type_version" id="type_version_1""#,
        r#"name="metadata_keys""#,
        r#"name="metadata_values""#,
    ] {
        assert!(html.contains(field), "{field}: {html}");
    }
    assert!(
        html.contains(r#"<form method="post" action="/orgs/testorg/destinations/environments""#)
    );
    for field in [r#"name="description""#, r#"name="sort_order" value="0""#] {
        assert!(html.contains(field), "{field}");
    }
    assert!(html.contains(r#"<option value="forest/terraform@2">forest/terraform v2 — Terraform workspace</option>"#), "{html}");
    assert!(html.contains(r#"id="type-fields-1" class="hidden space-y-2""#));
    assert!(html.contains(r#"id="extra-fields-1" class="hidden space-y-1""#));
    assert!(html.contains(r#"<script type="application/json" id="destination-types">"#));
    assert!(
        html.contains(r#"href="/orgs/testorg/destinations/detail?name=app%20prod%26eu""#),
        "{html}"
    );
    assert!(html.contains("Other destinations"));
    assert!(!html.contains("revealed-api_token"));
    insta::assert_snapshot!(main_element(&html));
}

#[tokio::test]
async fn the_export_button_carries_the_json_it_copies() {
    let html = get(Setup::default(), LIST).await;
    assert!(html.contains(r#"data-name="app prod&amp;eu""#), "{html}");
    assert!(html.contains(r#"data-env="prod""#));
    assert!(
        html.contains(r#"data-type-org="forest" data-type-name="terraform" data-type-version="2""#),
        "{html}"
    );
    assert!(
        html.contains(r#"data-metadata="{&quot;account&quot;:&quot;1234&quot;,&quot;region&quot;:&quot;eu-west-1&quot;}""#),
        "{html}"
    );
    assert!(
        html.contains(r#"data-sensitive-keys="[&quot;api_token&quot;]""#),
        "{html}"
    );
    assert!(
        html.contains(r#"data-type-org="" data-type-name="" data-type-version="""#),
        "{html}"
    );
    assert!(
        !html.contains(r#"data-type-name="none""#) && !html.contains(r#"data-type-version="none""#),
        "{html}"
    );
    assert!(html.contains("3 keys"));
    assert!(html.contains("1 hidden"));
}

#[tokio::test]
async fn a_destination_type_cannot_close_the_data_block() {
    let html = get(
        Setup {
            types: vec![terraform_type("evil</script><script>alert(1)</script>")],
            ..Default::default()
        },
        LIST,
    )
    .await;
    let start = html
        .find(r#"<script type="application/json" id="destination-types">"#)
        .unwrap();
    let block = &html[start..];
    let block = &block[..block.find("</script>").unwrap() + "</script>".len()];
    let escaped = "evil</script><script>alert(1)</script>"
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    assert!(block.contains(&escaped), "{block}");
    assert!(!html.contains("<script>alert(1)"), "{html}");
    assert!(
        html.contains("evil&lt;/script&gt;&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
}

#[tokio::test]
async fn members_see_destinations_without_admin_controls() {
    let html = get(
        Setup {
            admin: false,
            ..Default::default()
        },
        LIST,
    )
    .await;
    assert_no_inline_behaviour(&html);
    assert!(html.contains("app-dev"));
    assert!(html.contains("export-dest"));
    assert!(
        !html.contains("<form method=\"post\" action=\"/orgs/testorg/destinations"),
        "{html}"
    );
    assert!(!html.contains("destination-types"));
    assert!(!html.contains("draggable"));
    assert!(!html.contains("env-drag-handle"));
}

#[tokio::test]
async fn the_create_forms_post_the_fields_the_handlers_read() {
    let (status, location) = post(
        "/orgs/testorg/destinations/create",
        "_csrf=test-csrf&environment=dev&type_organisation=forest&type_name=terraform&type_version=2&name=new&metadata_keys=region&metadata_values=eu&sensitive_keys=region",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/orgs/testorg/destinations");

    let (status, location) = post(
        "/orgs/testorg/destinations/create",
        "_csrf=test-csrf&environment=dev&type_organisation=&type_name=&type_version=&name=new&metadata_keys=a&metadata_values=b",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "an untyped create must deserialize"
    );
    assert_eq!(location, "/orgs/testorg/destinations");

    let (status, location) = post(
        "/orgs/testorg/destinations/environments",
        "_csrf=test-csrf&name=staging&description=&sort_order=0",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/orgs/testorg/destinations");
}

const DETAIL: &str = "/orgs/testorg/destinations/detail?name=app%20prod%26eu";

#[tokio::test]
async fn the_detail_page_keeps_its_form_and_never_renders_hidden_values() {
    let html = get(Setup::default(), DETAIL).await;
    assert_no_inline_behaviour(&html);
    assert!(html.contains("/static/js/destinations.js?v="), "{html}");
    assert!(
        html.contains(r#"action="/orgs/testorg/destinations/detail/update?name=app%20prod%26eu" id="metadata-form""#),
        "{html}"
    );
    assert!(
        html.contains(
            r#"data-reveal-url="/orgs/testorg/destinations/detail/reveal?name=app%20prod%26eu""#
        ),
        "{html}"
    );
    assert!(
        html.contains(r#"data-declared-sensitive="[&quot;api_token&quot;]""#),
        "{html}"
    );
    assert!(html.contains(r#"name="_csrf" value="test-csrf""#));
    assert!(html.contains(r#"name="preserve_sensitive_keys" value="api_token""#));
    assert!(
        html.contains(r#"name="metadata_keys" value="api_token" placeholder="key" readonly"#),
        "{html}"
    );
    assert!(html.contains(r#"name="metadata_values" value="eu-west-1""#));
    assert!(html.contains(r#"data-key="api_token""#));
    assert!(!html.contains("revealed-api_token"));
    assert!(html.contains("1 hidden"));
    assert!(html.contains("3 keys"));
    insta::assert_snapshot!(main_element(&html));
}

#[tokio::test]
async fn members_see_masked_metadata_and_no_form() {
    let html = get(
        Setup {
            admin: false,
            ..Default::default()
        },
        DETAIL,
    )
    .await;
    assert_no_inline_behaviour(&html);
    assert!(!html.contains("metadata-form"));
    assert!(!html.contains("destinations.js"));
    assert!(html.contains("••••••••"));
    assert!(!html.contains("revealed-api_token"));
    insta::assert_snapshot!(main_element(&html));
}

#[tokio::test]
async fn an_untyped_destination_says_so() {
    let html = get(
        Setup::default(),
        "/orgs/testorg/destinations/detail?name=app-dev",
    )
    .await;
    assert!(html.contains("No type assigned"));
    assert!(!html.contains(">none<"), "{html}");
}

#[tokio::test]
async fn saving_metadata_redirects_back_to_the_destination() {
    let (status, location) = post(
        DETAIL.replace("/detail?", "/detail/update?").as_str(),
        "_csrf=test-csrf&metadata_keys=api_token&metadata_values=&preserve_sensitive_keys=api_token&metadata_keys=region&metadata_values=eu",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location,
        "/orgs/testorg/destinations/detail?name=app%20prod%26eu"
    );
}
