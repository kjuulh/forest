use forest_sdk_codegen::{Codegen, CodegenLanguage, CodegenOptions};

#[allow(dead_code)]
#[path = "generated/understory_lambda.rs"]
mod generated;

const FIXTURE: &str = include_str!("fixtures/understory_lambda.openapi.json");
const GENERATED_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/generated/understory_lambda.rs"
);

fn generate() -> String {
    Codegen {
        options: CodegenOptions {
            destination: String::new(),
            language: CodegenLanguage::Rust,
        },
    }
    .generate(FIXTURE)
    .expect("codegen")
}

#[test]
fn the_compiled_module_is_current() {
    let fresh = generate();
    if std::env::var_os("UPDATE_GENERATED").is_some() {
        std::fs::write(GENERATED_PATH, &fresh).unwrap();
        return;
    }
    let squash = |s: &str| {
        let compact: String = s.split_whitespace().collect();
        compact
            .replace(",)", ")")
            .replace(",}", "}")
            .replace(",]", "]")
    };
    let on_disk = std::fs::read_to_string(GENERATED_PATH).unwrap();
    assert_eq!(
        squash(&on_disk),
        squash(&fresh),
        "tests/generated/understory_lambda.rs is stale; rerun with UPDATE_GENERATED=1"
    );
}

#[test]
fn a_spec_that_omits_every_optional_block_decodes() {
    let spec: generated::Spec = serde_json::from_value(serde_json::json!({
        "name": "hello",
        "domain": "developer-platform",
        "image": {"repository": "hello", "tag": "abc"},
    }))
    .expect("an omitted block must fall back to its defaults");

    assert!(!spec.queue.enabled);
    assert_eq!(spec.queue.batch_size, 10);
    assert_eq!(spec.queue.max_receive_count, 4);
    assert!(spec.queue.report_batch_item_failures);
    assert!(spec.queue.subscriptions.is_empty());
    assert!(!spec.scheduler.enabled);
    assert_eq!(spec.http.api, "understory-internal-api");
    assert!(spec.http.routes.is_empty());
    assert!(spec.otel.enabled);
    assert_eq!(spec.otel.collector, "extension");
    assert!(spec.secrets.is_empty());
    assert!(spec.permissions.is_empty());
    assert_eq!(spec.architecture, "arm64");
    assert_eq!(spec.memory, 128);
    assert_eq!(spec.variables, serde_json::json!({}));
}

#[test]
fn a_partly_written_block_keeps_what_was_written_and_defaults_the_rest() {
    let spec: generated::Spec = serde_json::from_value(serde_json::json!({
        "name": "hello",
        "domain": "d",
        "image": {"repository": "hello", "tag": "abc"},
        "queue": {"enabled": true, "max_receive_count": 5},
        "secrets": ["development/api-keys"],
    }))
    .unwrap();

    assert!(spec.queue.enabled);
    assert_eq!(spec.queue.max_receive_count, 5);
    assert_eq!(spec.queue.batch_size, 10);
    assert_eq!(spec.secrets, vec!["development/api-keys".to_string()]);
}

#[test]
fn a_field_with_no_default_is_still_required() {
    let missing_repository = serde_json::from_value::<generated::Spec>(serde_json::json!({
        "name": "hello",
        "domain": "d",
        "image": {"tag": "abc"},
    }));
    assert!(missing_repository.is_err());

    let missing_name = serde_json::from_value::<generated::Spec>(serde_json::json!({
        "domain": "d",
        "image": {"repository": "hello", "tag": "abc"},
    }));
    assert!(missing_name.is_err());
}

#[test]
fn a_struct_with_a_required_member_gets_no_default() {
    let generated = generate();
    assert!(!generated.contains("impl Default for Image"));
    assert!(generated.contains("impl Default for Queue"));
    assert!(generated.contains("impl Default for Otel"));
}
