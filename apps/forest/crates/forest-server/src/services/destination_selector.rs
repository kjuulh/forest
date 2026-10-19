use anyhow::Context;

pub fn matches(pattern: &str, destination_name: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }

    match regex::Regex::new(pattern) {
        Ok(re) => re.is_match(destination_name),
        Err(_) => pattern == destination_name,
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeploymentItem {
    pub env: String,

    pub destination: String,

    #[serde(default)]
    pub destination_type: String,

    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

const ITEM_RECORD_SUFFIX: &str = "forest/config.json";

const MAX_SERIALIZED_BYTES: usize = 256 * 1024;

pub fn parse_declaration(
    files: &[(std::path::PathBuf, String)],
) -> anyhow::Result<Vec<DeploymentItem>> {
    let items: Vec<DeploymentItem> = parse_items(files)?
        .into_iter()
        .map(|parsed| parsed.item)
        .collect();

    let serialized = serde_json::to_vec(&items).context("serialize the declaration")?;
    if serialized.len() > MAX_SERIALIZED_BYTES {
        anyhow::bail!(
            "this artifact's deployment declaration is {} bytes, over the {} byte limit ({} items). Storing it would make every release of this project read it.",
            serialized.len(),
            MAX_SERIALIZED_BYTES,
            items.len(),
        );
    }

    Ok(items)
}

pub fn config_for_destination(
    items: &[DeploymentItem],
    environment: &str,
    destination_name: &str,
    destination_type: &str,
) -> std::collections::HashMap<String, String> {
    let Some(item) = items
        .iter()
        .find(|item| item.env == environment && selects(item, destination_name, destination_type))
    else {
        return std::collections::HashMap::new();
    };

    let Some(serde_json::Value::Object(config)) = item.config.as_ref() else {
        return std::collections::HashMap::new();
    };

    config
        .iter()
        .filter_map(|(key, value)| {
            let flattened = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some((key.clone(), flattened))
        })
        .collect()
}

pub async fn recorded_declaration(
    db: &sqlx::PgPool,
    artifact: &uuid::Uuid,
) -> anyhow::Result<Option<Vec<DeploymentItem>>> {
    let recorded = sqlx::query_scalar!(
        "SELECT deployment_items FROM annotations WHERE artifact_id = $1",
        artifact,
    )
    .fetch_optional(db)
    .await
    .context("read the recorded declaration")?
    .flatten()
    .filter(|value| !value.is_null());

    match recorded {
        Some(value) => Ok(Some(
            serde_json::from_value(value).context("parse the recorded declaration")?,
        )),
        None => Ok(None),
    }
}

pub async fn projects_widened_by(
    db: &sqlx::PgPool,
    organisation: &str,
    environment: &str,
    destination_type: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (p.id)
            p.project AS "project!",
            a.deployment_items
        FROM projects p
        JOIN annotations a ON a.project_id = p.id
        WHERE p.organisation = $1
          AND EXISTS (
            SELECT 1
            FROM release_states rs
            JOIN destinations d ON d.id = rs.destination_id
            WHERE rs.project_id = p.id
              AND d.organisation = $1
              AND d.environment = $2
          )
        ORDER BY p.id, a.created DESC
        "#,
        organisation,
        environment,
    )
    .fetch_all(db)
    .await
    .context("read the projects releasing into this environment")?;

    let mut widened = Vec::new();
    for row in rows {
        let items: Option<Vec<DeploymentItem>> = match row.deployment_items {
            Some(value) if !value.is_null() => match serde_json::from_value(value) {
                Ok(items) => Some(items),
                Err(e) => {
                    tracing::warn!(
                        project = row.project,
                        "unreadable recorded declaration: {e}"
                    );
                    continue;
                }
            },
            _ => None,
        };

        if widened_by_new_destination(items.as_deref(), environment, destination_type) {
            widened.push(format!("{organisation}/{}", row.project));
        }
    }

    widened.sort();
    Ok(widened)
}

pub fn type_matches(declared: &str, actual: &str) -> bool {
    let declared = declared.trim();
    let actual = actual.trim();

    if declared.is_empty() || actual.is_empty() {
        return true;
    }

    declared == actual
}

pub fn selects(item: &DeploymentItem, destination_name: &str, destination_type: &str) -> bool {
    matches(&item.destination, destination_name)
        && type_matches(&item.destination_type, destination_type)
}

pub fn narrow_to_declared<T>(
    candidates: Vec<T>,
    items: &[DeploymentItem],
    environment: &str,
    describe: impl Fn(&T) -> (&str, &str),
) -> Result<Vec<T>, String> {
    let declared: Vec<&DeploymentItem> = items
        .iter()
        .filter(|item| item.env == environment)
        .collect();
    if declared.is_empty() {
        return narrow_undeclared(candidates, items, environment, describe);
    }

    for item in &declared {
        if !candidates.iter().any(|candidate| {
            let (name, destination_type) = describe(candidate);
            selects(item, name, destination_type)
        }) {
            tracing::warn!(
                environment,
                selector = item.destination,
                destination_type = item.destination_type,
                "declared destination selector matches nothing in this environment"
            );
        }
    }

    let available: Vec<String> = candidates
        .iter()
        .map(|candidate| describe_one(&describe, candidate))
        .collect();

    let mut wrong_kind: Vec<String> = Vec::new();

    let selected: Vec<T> = candidates
        .into_iter()
        .filter(|candidate| {
            let (name, destination_type) = describe(candidate);

            if declared
                .iter()
                .any(|item| selects(item, name, destination_type))
            {
                return true;
            }

            if declared.iter().any(|item| matches(&item.destination, name)) {
                tracing::info!(
                    environment,
                    destination = name,
                    destination_type,
                    "not scheduled: this project declares no component of this destination's type"
                );
                wrong_kind.push(format!("{name} ({destination_type})"));
            }

            false
        })
        .collect();

    if selected.is_empty() {
        let selectors: Vec<String> = declared
            .iter()
            .map(|item| describe_declared(item))
            .collect();

        let mut message = format!(
            "environment '{environment}' holds {} ({}), and this project declared [{}] for it, which match none of them",
            match available.len() {
                1 => "1 destination".to_string(),
                n => format!("{n} destinations"),
            },
            available.join(", "),
            selectors.join(", "),
        );

        if !wrong_kind.is_empty() {
            message.push_str(&format!(
                " — {} match by name but are of a type this project declares nothing for",
                wrong_kind.join(", "),
            ));
        }

        return Err(message);
    }

    Ok(selected)
}

pub fn rendered_types(items: &[DeploymentItem]) -> Option<std::collections::BTreeSet<&str>> {
    if items.is_empty()
        || items
            .iter()
            .any(|item| item.destination_type.trim().is_empty())
    {
        return None;
    }

    Some(
        items
            .iter()
            .map(|item| item.destination_type.trim())
            .collect(),
    )
}

fn narrow_undeclared<T>(
    candidates: Vec<T>,
    items: &[DeploymentItem],
    environment: &str,
    describe: impl Fn(&T) -> (&str, &str),
) -> Result<Vec<T>, String> {
    let Some(rendered) = rendered_types(items) else {
        return Ok(candidates);
    };

    let available: Vec<String> = candidates
        .iter()
        .map(|candidate| describe_one(&describe, candidate))
        .collect();

    let selected: Vec<T> = candidates
        .into_iter()
        .filter(|candidate| {
            let (name, destination_type) = describe(candidate);
            let keep = rendered
                .iter()
                .any(|rendered| type_matches(rendered, destination_type));
            if !keep {
                tracing::info!(
                    environment,
                    destination = name,
                    destination_type,
                    "not scheduled: this project declares nothing for the environment and renders nothing of this destination's type"
                );
            }
            keep
        })
        .collect();

    if selected.is_empty() && !available.is_empty() {
        return Err(format!(
            "this project declares no destinations for environment '{environment}', and none of what it holds ({}) is of a type the artifact renders ({})",
            available.join(", "),
            rendered.into_iter().collect::<Vec<_>>().join(", "),
        ));
    }

    Ok(selected)
}

pub fn widened_by_new_destination(
    items: Option<&[DeploymentItem]>,
    environment: &str,
    destination_type: &str,
) -> bool {
    let Some(items) = items else {
        return true;
    };

    if items.iter().any(|item| item.env == environment) {
        return false;
    }

    match rendered_types(items) {
        None => true,
        Some(rendered) => rendered
            .iter()
            .any(|rendered| type_matches(rendered, destination_type)),
    }
}

fn describe_one<T>(describe: &impl Fn(&T) -> (&str, &str), candidate: &T) -> String {
    let (name, destination_type) = describe(candidate);
    if destination_type.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({destination_type})")
    }
}

fn describe_declared(item: &DeploymentItem) -> String {
    if item.destination_type.is_empty() {
        item.destination.clone()
    } else {
        format!("{} ({})", item.destination, item.destination_type)
    }
}

pub struct ParsedItem {
    pub root: std::path::PathBuf,
    pub item: DeploymentItem,
}

pub fn parse_items(files: &[(std::path::PathBuf, String)]) -> anyhow::Result<Vec<ParsedItem>> {
    let mut parsed = Vec::new();

    for (path, content) in files {
        if !path.ends_with(ITEM_RECORD_SUFFIX) {
            continue;
        }

        let item: DeploymentItem = serde_json::from_str(content)
            .with_context(|| format!("parse the deployment item recorded at {}", path.display()))?;

        if item.env.is_empty() || item.destination.is_empty() {
            anyhow::bail!(
                "the deployment item recorded at {} names no {}",
                path.display(),
                if item.env.is_empty() {
                    "environment"
                } else {
                    "destination"
                },
            );
        }

        let root = path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        parsed.push(ParsedItem { root, item });
    }

    parsed.sort_by_key(|p| std::cmp::Reverse(p.root.components().count()));
    Ok(parsed)
}

pub fn attribute_files<'a>(
    files: &'a [(std::path::PathBuf, String)],
    items: &[ParsedItem],
) -> Vec<(&'a str, String, String)> {
    let mut attributed = Vec::new();

    for (path, _) in files {
        let Some(found) = items.iter().find(|p| path.starts_with(&p.root)) else {
            continue;
        };
        let Some(name) = path.to_str() else {
            continue;
        };

        attributed.push((name, found.item.env.clone(), found.item.destination.clone()));
    }

    attributed
}

pub fn selectors_for_env<'a>(items: &'a [DeploymentItem], env: &str) -> Vec<&'a str> {
    let mut selectors: Vec<&str> = items
        .iter()
        .filter(|item| item.env == env)
        .map(|item| item.destination.as_str())
        .collect();

    selectors.sort_unstable();
    selectors.dedup();
    selectors
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        DeploymentItem, config_for_destination, matches, narrow_to_declared, parse_declaration,
        selectors_for_env, type_matches, widened_by_new_destination,
    };

    fn item_record(env: &str, selector: &str) -> (PathBuf, String) {
        (
            PathBuf::from(format!(
                "{env}/{selector}/forest/generic@1/forest/config.json"
            )),
            format!(
                r#"{{"env":"{env}","destination":"{selector}",
                    "destination_type":"forest/generic@1","component":null,
                    "config":{{"service":"canopy_hubspot_ingest"}}}}"#
            ),
        )
    }

    #[test]
    fn an_anchored_selector_matches_its_place_and_nothing_else() {
        let selector = "^data-prod/.*$";

        assert!(matches(
            selector,
            "data-prod/eu-west-1/infrastructure-data-ecs"
        ));

        assert!(!matches(selector, "data"));

        assert!(!matches(selector, "data-dev/eu-west-1/infrastructure-data"));
    }

    #[test]
    fn an_unanchored_selector_is_left_unanchored() {
        assert!(matches("data-prod", "data-prod/eu-west-1/x"));
        assert!(matches("data-prod", "legacy-data-prod-mirror"));
    }

    #[test]
    fn an_unparseable_pattern_falls_back_to_equality() {
        let broken = "^(data-prod";

        assert!(matches(broken, broken));
        assert!(!matches(broken, "data-prod/eu-west-1/x"));
        assert!(!matches(broken, "anything-else"));
    }

    #[test]
    fn a_truncated_selector_still_matches_what_the_full_one_would() {
        assert!(matches("^data-prod", "data-prod/eu-west-1/x"));
        assert!(!matches("^data-prod", "data"));
    }

    #[test]
    fn the_declaration_keeps_the_selector_whole() {
        let files = vec![item_record("data", "^data-prod/.*$")];

        let items = parse_declaration(&files).expect("parse");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].env, "data");
        assert_eq!(items[0].destination, "^data-prod/.*$");
        assert_eq!(items[0].destination_type, "forest/generic@1");
    }

    #[test]
    fn only_item_records_are_read() {
        let files = vec![
            item_record("data-dev", "^data-dev/.*$"),
            (
                PathBuf::from("data-dev/^data-dev/.*$/forest/generic@1/deployment.yaml"),
                "apiVersion: apps/v1\n".to_string(),
            ),
            (
                PathBuf::from("forest.cue"),
                "project: name: \"x\"\n".to_string(),
            ),
        ];

        let items = parse_declaration(&files).expect("parse");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].destination, "^data-dev/.*$");
    }

    #[test]
    fn no_item_records_is_an_empty_declaration_not_an_error() {
        let files = vec![(
            PathBuf::from("forest.cue"),
            "project: name: \"x\"\n".to_string(),
        )];

        assert!(parse_declaration(&files).expect("parse").is_empty());
    }

    #[test]
    fn an_unparseable_item_record_fails_the_whole_parse() {
        let files = vec![
            item_record("data-dev", "^data-dev/.*$"),
            (
                PathBuf::from("data/x/forest/generic@1/forest/config.json"),
                "{ not json".to_string(),
            ),
        ];

        let err = parse_declaration(&files).expect_err("must not silently skip");

        assert!(
            format!("{err:#}").contains("data/x/forest/generic@1/forest/config.json"),
            "error should name the file, got: {err:#}"
        );
    }

    #[test]
    fn an_oversized_declaration_is_refused_with_its_size() {
        let big = "x".repeat(300 * 1024);
        let files = vec![(
            PathBuf::from("data/sel/forest/generic@1/forest/config.json"),
            format!(
                r#"{{"env":"data","destination":"sel","destination_type":"t",
                    "config":{{"blob":"{big}"}}}}"#
            ),
        )];

        let err = parse_declaration(&files).expect_err("must be refused");
        assert!(format!("{err:#}").contains("262144"), "got: {err:#}");
    }

    #[test]
    fn selectors_are_scoped_to_the_stage_environment() {
        let items = vec![
            DeploymentItem {
                env: "data-dev".into(),
                destination: "^data-dev/.*$".into(),
                destination_type: "forest/generic@1".into(),
                config: None,
            },
            DeploymentItem {
                env: "data".into(),
                destination: "^data-prod/.*$".into(),
                destination_type: "forest/generic@1".into(),
                config: None,
            },
        ];

        assert_eq!(selectors_for_env(&items, "data-dev"), vec!["^data-dev/.*$"]);
        assert_eq!(selectors_for_env(&items, "data"), vec!["^data-prod/.*$"]);

        assert!(selectors_for_env(&items, "data-prod").is_empty());
    }

    fn item(env: &str, selector: &str, config: serde_json::Value) -> DeploymentItem {
        DeploymentItem {
            env: env.into(),
            destination: selector.into(),
            destination_type: "forest/generic@1".into(),
            config: Some(config),
        }
    }

    #[test]
    fn config_is_picked_by_the_same_selector_the_scheduler_used() {
        let items = vec![
            item(
                "data",
                "^data-prod/.*$",
                serde_json::json!({ "service": "canopy_hubspot_ingest" }),
            ),
            item(
                "data",
                "^data-stage/.*$",
                serde_json::json!({ "service": "wrong_one" }),
            ),
        ];

        let config =
            config_for_destination(&items, "data", "data-prod/eu-west-1/x", "forest/generic@1");

        assert_eq!(
            config.get("service").map(String::as_str),
            Some("canopy_hubspot_ingest"),
        );
    }

    #[test]
    fn a_destination_the_project_did_not_declare_gets_no_config() {
        let items = vec![item(
            "data",
            "^data-prod/.*$",
            serde_json::json!({ "service": "x" }),
        )];

        assert!(config_for_destination(&items, "data", "data", "forest/generic@1").is_empty());
    }

    #[test]
    fn the_environment_must_match_too() {
        let items = vec![item(
            "data-dev",
            "^data-.*$",
            serde_json::json!({ "service": "x" }),
        )];

        assert!(
            config_for_destination(&items, "data-prod", "data-prod/x", "forest/generic@1")
                .is_empty()
        );
    }

    #[test]
    fn scalars_are_stringified_and_nested_structure_is_dropped() {
        let items = vec![item(
            "e",
            "d",
            serde_json::json!({
                "service": "svc",
                "replicas": 3,
                "enabled": true,
                "nested": { "a": 1 },
                "list": [1, 2],
            }),
        )];

        let config = config_for_destination(&items, "e", "d", "forest/generic@1");

        assert_eq!(config.get("service").map(String::as_str), Some("svc"));
        assert_eq!(config.get("replicas").map(String::as_str), Some("3"));
        assert_eq!(config.get("enabled").map(String::as_str), Some("true"));
        assert!(!config.contains_key("nested"), "nested maps are dropped");
        assert!(!config.contains_key("list"), "arrays are dropped");
    }

    #[test]
    fn an_empty_selector_selects_nothing_not_everything() {
        assert!(!matches("", "data-prod/eu-west-1/x"));
        assert!(!matches("", "data"));
        assert!(!matches("", ""));
    }

    #[test]
    fn an_item_naming_no_selector_is_refused_at_annotate() {
        let files = vec![(
            PathBuf::from("data/x/forest/generic@1/forest/config.json"),
            r#"{"env":"data","destination":"","destination_type":"t"}"#.to_string(),
        )];

        let err = parse_declaration(&files).expect_err("an empty selector must be refused");
        let msg = format!("{err:#}");
        assert!(msg.contains("names no destination"), "got: {msg}");
        assert!(
            msg.contains("config.json"),
            "should name the file, got: {msg}"
        );
    }

    #[test]
    fn an_item_naming_no_environment_is_refused_at_annotate() {
        let files = vec![(
            PathBuf::from("data/x/forest/generic@1/forest/config.json"),
            r#"{"env":"","destination":"^data-prod/.*$","destination_type":"t"}"#.to_string(),
        )];

        let err = parse_declaration(&files).expect_err("an empty env must be refused");
        assert!(format!("{err:#}").contains("names no environment"));
    }

    #[test]
    fn an_empty_selector_already_recorded_still_selects_nothing() {
        let items = vec![DeploymentItem {
            env: "data".into(),
            destination: String::new(),
            destination_type: "t".into(),
            config: Some(serde_json::json!({ "service": "x" })),
        }];

        assert!(config_for_destination(&items, "data", "anything", "t").is_empty());
        assert!(!matches(&items[0].destination, "anything"));
    }

    fn dest(name: &str, destination_type: &str) -> (String, String) {
        (name.to_string(), destination_type.to_string())
    }

    fn describe(d: &(String, String)) -> (&str, &str) {
        (d.0.as_str(), d.1.as_str())
    }

    fn declared(env: &str, selector: &str, destination_type: &str) -> DeploymentItem {
        DeploymentItem {
            env: env.into(),
            destination: selector.into(),
            destination_type: destination_type.into(),
            config: None,
        }
    }

    #[test]
    fn a_terraform_item_does_not_schedule_an_ecs_destination() {
        let items = vec![declared("platform-dev", "dev.*", "forest/terraform@1")];

        let candidates = vec![
            dest(
                "dev/eu-west-1/infrastructure-platform",
                "forest/terraform@1",
            ),
            dest(
                "platform-dev/eu-west-1/infrastructure-platform",
                "forest/generic@1",
            ),
        ];

        assert!(matches(
            "dev.*",
            "platform-dev/eu-west-1/infrastructure-platform"
        ));

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("some match");

        assert_eq!(
            selected.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
            vec!["dev/eu-west-1/infrastructure-platform"],
            "a terraform declaration must not reach an ECS place",
        );
    }

    #[test]
    fn an_ecs_item_does_not_schedule_a_terraform_destination() {
        let items = vec![declared(
            "platform-dev",
            ".*infrastructure-platform",
            "forest/generic@1",
        )];

        let candidates = vec![
            dest(
                "dev/eu-west-1/infrastructure-platform",
                "forest/terraform@1",
            ),
            dest(
                "platform-dev/eu-west-1/infrastructure-platform",
                "forest/generic@1",
            ),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("some match");

        assert_eq!(
            selected.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
            vec!["platform-dev/eu-west-1/infrastructure-platform"],
        );
    }

    #[test]
    fn a_project_declaring_both_types_schedules_both() {
        let items = vec![
            declared("platform-dev", "^dev/.*$", "forest/terraform@1"),
            declared("platform-dev", "^platform-dev/.*$", "forest/generic@1"),
        ];

        let candidates = vec![
            dest(
                "dev/eu-west-1/infrastructure-platform",
                "forest/terraform@1",
            ),
            dest(
                "platform-dev/eu-west-1/infrastructure-platform",
                "forest/generic@1",
            ),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("both match");

        assert_eq!(selected.len(), 2, "got: {selected:?}");
    }

    #[test]
    fn a_destination_of_an_undeclared_type_is_skipped_not_failed() {
        let items = vec![declared("platform-dev", ".*", "forest/generic@1")];

        let candidates = vec![
            dest("a", "forest/generic@1"),
            dest("b", "forest/terraform@1"),
        ];

        let selected = narrow_to_declared(candidates, &items, "platform-dev", describe)
            .expect("the declared one matches, so this is not an error");

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, "a");
    }

    #[test]
    fn a_type_only_mismatch_says_it_is_a_type_mismatch() {
        let items = vec![declared("platform-dev", "^dev/.*$", "forest/terraform@1")];

        let candidates = vec![dest(
            "dev/eu-west-1/infrastructure-platform",
            "forest/generic@1",
        )];

        let err = narrow_to_declared(candidates, &items, "platform-dev", describe)
            .expect_err("nothing is selectable");

        assert!(err.contains("forest/terraform@1"), "got: {err}");
        assert!(err.contains("forest/generic@1"), "got: {err}");
        assert!(
            err.contains("a type this project declares nothing for"),
            "got: {err}",
        );
    }

    #[test]
    fn an_item_with_no_recorded_type_still_gates_on_the_name_alone() {
        let items = vec![declared("platform-dev", "^dev/.*$", "")];

        let candidates = vec![
            dest(
                "dev/eu-west-1/infrastructure-platform",
                "forest/terraform@1",
            ),
            dest(
                "platform-dev/eu-west-1/infrastructure-platform",
                "forest/generic@1",
            ),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("some match");

        assert_eq!(
            selected.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(),
            vec!["dev/eu-west-1/infrastructure-platform"],
            "the name selector still applies; only the type check is skipped",
        );
    }

    #[test]
    fn a_candidate_with_no_known_type_is_not_filtered_out() {
        assert!(type_matches("", "forest/generic@1"));
        assert!(type_matches("forest/generic@1", ""));
        assert!(type_matches("", ""));
    }

    #[test]
    fn types_compare_whole_including_the_version() {
        assert!(type_matches("forest/terraform@1", "forest/terraform@1"));
        assert!(!type_matches("forest/terraform@1", "forest/terraform@2"));
        assert!(!type_matches("forest/terraform@1", "forest/generic@1"));
        assert!(!type_matches(
            "forest/terraform@1",
            "understory/terraform@1"
        ));
    }

    #[test]
    fn nothing_declared_for_the_environment_still_fans_out_across_its_own_kinds() {
        let items = vec![
            declared("some-other-env", "^dev/.*$", "forest/terraform@1"),
            declared("some-other-env", "^x$", "forest/generic@1"),
        ];

        let candidates = vec![
            dest("a", "forest/generic@1"),
            dest("b", "forest/terraform@1"),
            dest("c", "forest/generic@1"),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("fans out");

        assert_eq!(selected.len(), 3, "every destination of a rendered type");
    }

    #[test]
    fn a_new_kind_of_destination_does_not_widen_an_undeclared_project() {
        let items = vec![declared("prod", "^prod/.*$", "forest/generic@1")];

        let candidates = vec![
            dest("dev/eu-west-1/development", "forest/generic@1"),
            dest("dev/eu-west-1/core", "forest/terraform@1"),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "dev", describe).expect("the ECS one remains");

        assert_eq!(
            selected,
            vec![dest("dev/eu-west-1/development", "forest/generic@1")]
        );
    }

    #[test]
    fn an_undeclared_environment_holding_only_other_kinds_fails_loudly() {
        let items = vec![declared("prod", "^prod/.*$", "forest/generic@1")];
        let candidates = vec![dest("dev/eu-west-1/core", "forest/terraform@1")];

        let error = narrow_to_declared(candidates, &items, "dev", describe)
            .expect_err("nothing of a rendered type");

        assert!(
            error.contains("declares no destinations for environment 'dev'"),
            "{error}"
        );
        assert!(
            error.contains("dev/eu-west-1/core (forest/terraform@1)"),
            "{error}"
        );
        assert!(error.contains("forest/generic@1"), "{error}");
    }

    #[test]
    fn an_artifact_with_no_items_still_fans_out_to_everything() {
        let candidates = vec![
            dest("a", "forest/generic@1"),
            dest("b", "forest/terraform@1"),
        ];

        let selected = narrow_to_declared(candidates, &[], "dev", describe).expect("no filter");

        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn an_untyped_item_anywhere_disables_the_type_filter() {
        let items = vec![
            declared("prod", "^prod/.*$", "forest/generic@1"),
            declared("prod", "^prod-tf$", ""),
        ];
        let candidates = vec![
            dest("a", "forest/generic@1"),
            dest("b", "forest/terraform@1"),
        ];

        let selected = narrow_to_declared(candidates, &items, "dev", describe).expect("no filter");

        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn a_new_destination_widens_only_undeclared_projects_that_render_its_kind() {
        let ecs_only = vec![declared("prod", "^prod/.*$", "forest/generic@1")];

        assert!(widened_by_new_destination(
            Some(&ecs_only),
            "dev",
            "forest/generic@1"
        ));
        assert!(!widened_by_new_destination(
            Some(&ecs_only),
            "dev",
            "forest/terraform@1"
        ));
    }

    #[test]
    fn a_project_declaring_the_environment_is_never_widened() {
        let declaring = vec![declared("dev", ".*", "forest/terraform@1")];

        assert!(!widened_by_new_destination(
            Some(&declaring),
            "dev",
            "forest/terraform@1"
        ));
    }

    #[test]
    fn a_project_that_cannot_say_what_it_renders_is_widened() {
        assert!(widened_by_new_destination(
            None,
            "dev",
            "forest/terraform@1"
        ));
        assert!(widened_by_new_destination(
            Some(&[]),
            "dev",
            "forest/terraform@1"
        ));
        assert!(widened_by_new_destination(
            Some(&[declared("prod", "^p$", "")]),
            "dev",
            "forest/terraform@1",
        ));
    }

    #[test]
    fn an_undeclared_candidate_with_no_known_type_is_kept() {
        let items = vec![declared("prod", "^prod/.*$", "forest/generic@1")];
        let candidates = vec![dest("a", "")];

        let selected = narrow_to_declared(candidates, &items, "dev", describe).expect("kept");

        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn config_comes_from_the_item_that_selected_the_destination() {
        let items = vec![
            DeploymentItem {
                env: "platform-dev".into(),
                destination: ".*infrastructure-platform".into(),
                destination_type: "forest/terraform@1".into(),
                config: Some(serde_json::json!({ "service": "terraform_one" })),
            },
            DeploymentItem {
                env: "platform-dev".into(),
                destination: ".*infrastructure-platform".into(),
                destination_type: "forest/generic@1".into(),
                config: Some(serde_json::json!({ "service": "ecs_one" })),
            },
        ];

        let config = config_for_destination(
            &items,
            "platform-dev",
            "platform-dev/eu-west-1/infrastructure-platform",
            "forest/generic@1",
        );

        assert_eq!(config.get("service").map(String::as_str), Some("ecs_one"));
    }

    #[test]
    fn the_fungus_declaration_reaches_one_destination_of_each_kind() {
        let files = vec![
            (
                PathBuf::from("platform-dev/^dev/.*$/forest/terraform@1/forest/config.json"),
                r#"{"env":"platform-dev","destination":"^dev/.*$",
                    "destination_type":"forest/terraform@1","component":null,
                    "config":{"name":"fungus"}}"#
                    .to_string(),
            ),
            (
                PathBuf::from("platform-dev/^platform-dev/.*$/forest/generic@1/forest/config.json"),
                r#"{"env":"platform-dev","destination":"^platform-dev/.*$",
                    "destination_type":"forest/generic@1","component":null,
                    "config":{"service":"fungus"}}"#
                    .to_string(),
            ),
        ];

        let items = parse_declaration(&files).expect("the real records parse");

        let candidates = vec![
            dest(
                "dev/eu-west-1/infrastructure-platform",
                "forest/terraform@1",
            ),
            dest(
                "platform-dev/eu-west-1/infrastructure-platform",
                "forest/generic@1",
            ),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("both match");

        assert_eq!(selected.len(), 2);

        assert_eq!(
            config_for_destination(
                &items,
                "platform-dev",
                "platform-dev/eu-west-1/infrastructure-platform",
                "forest/generic@1",
            )
            .get("service")
            .map(String::as_str),
            Some("fungus"),
        );
        assert_eq!(
            config_for_destination(
                &items,
                "platform-dev",
                "dev/eu-west-1/infrastructure-platform",
                "forest/terraform@1",
            )
            .get("name")
            .map(String::as_str),
            Some("fungus"),
        );
    }

    #[test]
    fn the_unanchored_fungus_selector_no_longer_reaches_the_ecs_place() {
        let items = vec![
            declared("platform-dev", "dev/.*", "forest/terraform@1"),
            declared("platform-dev", "^platform-dev/.*$", "forest/generic@1"),
        ];

        assert!(
            matches("dev/.*", "platform-dev/eu-west-1/infrastructure-platform"),
            "the selector really does match the ECS place by name",
        );

        let ecs = vec![dest(
            "platform-dev/eu-west-1/infrastructure-platform",
            "forest/generic@1",
        )];

        let selected = narrow_to_declared(ecs, &items, "platform-dev", describe).expect("matches");
        assert_eq!(selected.len(), 1);

        let terraform_only = vec![declared("platform-dev", "dev/.*", "forest/terraform@1")];
        let ecs = vec![dest(
            "platform-dev/eu-west-1/infrastructure-platform",
            "forest/generic@1",
        )];

        narrow_to_declared(ecs, &terraform_only, "platform-dev", describe)
            .expect_err("a terraform-only declaration has no business here");
    }
}
