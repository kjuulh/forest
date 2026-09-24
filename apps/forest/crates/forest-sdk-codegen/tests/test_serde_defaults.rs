//! A consumer's `config` reaches a component binary without being unified
//! with `#Spec`, so the generated types have to supply every CUE default
//! themselves. These tests generate code for a real component
//! (forest-modules' understory/lambda, whose spec leans on every kind of
//! default), COMPILE it — it is the `generated` module below — and decode
//! specs that leave blocks out. understory-io/forest#287.

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

/// The compiled module must be what the generator produces today, or the
/// decode tests below are testing stale output. Insensitive to formatting, so
/// neither rustfmt over the tests directory nor a different rustfmt version
/// on CI can break it: whitespace is dropped, and so is a comma directly
/// before a closing bracket, because that is what rustfmt adds when it wraps
/// a call or a struct literal onto several lines.
/// Regenerate with `UPDATE_GENERATED=1 cargo test -p forest-sdk-codegen`.
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
    // Before #287: `missing field queue` (a struct whose members all default),
    // then `missing field secrets` (a `[...string] | *[]` list).
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
    // `{[string]: string} | *{}` is an opaque object; omitted means `{}`, not Null.
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
    // `image.repository` and `name` have no default; relaxing them would turn
    // a missing value into an empty string that fails somewhere far away.
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
    // `Image` has required `repository`, so there is no `impl Default for
    // Image` and `image` stays required on Spec.
    let generated = generate();
    assert!(!generated.contains("impl Default for Image"));
    assert!(generated.contains("impl Default for Queue"));
    assert!(generated.contains("impl Default for Otel"));
}
