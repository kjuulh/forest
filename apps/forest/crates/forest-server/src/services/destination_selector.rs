//! Matching a project's declared destination against a real one.
//!
//! A project's `forest.cue` names the places it releases to as *selectors* rather
//! than names:
//!
//! ```cue
//! env: "data-prod": destinations: [{
//!     destination: "^data-prod/.*$"
//!     type:        "forest/generic@1"
//! }]
//! ```
//!
//! `release prepare` renders that verbatim into each deployment item's
//! `forest/config.json`, and two things downstream have to agree on what it
//! matches: the scheduler, deciding which destinations a deploy stage releases
//! to, and the provider, deciding which config to hand the destination it is
//! releasing to. They used to disagree — the provider matched the selector as a
//! regex while the scheduler ignored it entirely and released to every
//! destination in the environment. An ECS service artifact was scheduled at a
//! shiitake slice registry, failed there, and the provider was left correctly
//! refusing to supply config for a destination it could see did not match.
//!
//! Hence one function, called by both.

use anyhow::Context;

/// Does `pattern`, as declared by a project, select the destination named
/// `destination_name`?
///
/// The pattern is a **regex**, not a glob and not a literal, matched
/// **unanchored**. Anchoring is the author's to choose: the selectors in the
/// field carry their own `^` and `$`, so anchoring here would silently change
/// what they mean. Unanchored `is_match` is also exactly what
/// `genericv1::release_config` has always done, and the scheduler must not be a
/// hair narrower or wider than the thing that supplies config — narrower skips a
/// destination that would have been configured, wider is the bug this exists to
/// fix.
///
/// An unparseable pattern falls back to equality rather than to matching
/// everything. A project with a broken selector then filters to nothing and its
/// stage fails loudly, which is the outcome we want: the alternative reading of
/// "this pattern is meaningless" is "so release everywhere", and that is how an
/// artifact ends up somewhere nobody asked for.
pub fn matches(pattern: &str, destination_name: &str) -> bool {
    // An empty pattern selects nothing. `Regex::new("")` is perfectly valid and
    // matches *every* string, so without this an empty selector would quietly
    // select an entire environment — the silent widening this module exists to
    // stop, arriving through the one input nobody writes on purpose.
    if pattern.is_empty() {
        return false;
    }

    match regex::Regex::new(pattern) {
        Ok(re) => re.is_match(destination_name),
        Err(_) => pattern == destination_name,
    }
}

/// One rendered deployment item — what the project declared for one
/// (environment, destination selector, destination type) triple, plus the config
/// it contributes about itself.
///
/// This is the server's record of `release prepare`'s output, parsed once at
/// annotate time and stored on the annotation. It deliberately mirrors the CLI's
/// `DeploymentItem` rather than sharing it: the CLI's is a local render type that
/// also carries a component reference, and this one is a persisted record whose
/// shape is a compatibility surface.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeploymentItem {
    pub env: String,

    /// The selector, verbatim as the project wrote it. A regex over destination
    /// names — see [`matches`].
    pub destination: String,

    #[serde(default)]
    pub destination_type: String,

    /// Whatever the project contributes about itself for this destination.
    /// Absent is normal: most destination types carry everything they need.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Suffix identifying a rendered item's own record within the uploaded files.
const ITEM_RECORD_SUFFIX: &str = "forest/config.json";

/// The largest declaration we will store, serialized.
///
/// Only the items are stored, never the manifests beside them, so the realistic
/// size is a few hundred bytes per (env × destination × type) triple. The cap is
/// here so that a project which finds a way to make it enormous gets a clear
/// error at annotate time instead of a row whose cost nobody predicted.
const MAX_SERIALIZED_BYTES: usize = 256 * 1024;

/// Parse the project's declaration out of the artifact's uploaded deployment
/// files.
///
/// Takes the files rather than fetching them so that the caller owns the read
/// (and so this is testable without a database or an object store).
///
/// **An item record that will not parse is an error, not a skip.** A partial
/// declaration is a silently narrower filter — or, if every item fails, a
/// silently absent one — and scheduling against either is the failure this
/// column exists to prevent. Annotate is also the right place to fail: it is
/// loud, immediate, in the CI log of whoever just pushed, and retryable, where a
/// deploy stage failing on it lands twenty minutes later behind a wait gate.
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

/// The config a declaration contributes for one destination, flattened for a
/// provider that receives a flat string map.
///
/// The item is picked by the same [`selects`] the scheduler used, so the config a
/// destination receives and the reason it was chosen cannot come apart. Nested
/// structure is dropped rather than encoded as JSON into a value someone would
/// have to guess the shape of.
///
/// Type included, and it matters here as much as in the scheduler: a project
/// declaring two items for one environment — an ECS one and a terraform one, as
/// `fungus` does — has two records whose selectors can both match a given name,
/// and picking the first by name alone hands the ECS destination the terraform
/// item's config.
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

/// The declaration recorded for an artifact, if one was.
///
/// `None` means the artifact was annotated before the column existed — callers
/// must fall back to whatever they did then, not treat it as "declared nothing".
/// `Some(vec![])` is a statement: prepared, and declared nothing.
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

/// Does a project's declared destination *type* cover a destination of type
/// `actual`?
///
/// Both are the `organisation/name@version` spelling — `forest/terraform@1` —
/// the one `destination create --type` takes and the one `release prepare`
/// renders into each item record, so they compare directly.
///
/// **Blank on either side means "do not filter on type."** That is the
/// compatibility hinge, and it has two real sources: an artifact annotated
/// before item records carried a type at all (`destination_type` is
/// `#[serde(default)]`), and a caller that has no type to offer. Neither is a
/// statement about kind, and reading absence as "matches nothing" would fail
/// stages for projects that never said anything wrong.
pub fn type_matches(declared: &str, actual: &str) -> bool {
    let declared = declared.trim();
    let actual = actual.trim();

    if declared.is_empty() || actual.is_empty() {
        return true;
    }

    declared == actual
}

/// Does one declared item select this destination? **Place and kind, both.**
///
/// The name selector says *where*; the item's `type` says *what it renders*. A
/// project that declared `forest/terraform@1` for `^dev/.*$` has said it ships
/// terraform to the terraform places called `dev/…` — it has not volunteered to
/// have a terraform plan run at an ECS service that happens to share the name
/// shape, and it has not asked for an ECS rollout either.
///
/// Matching on the name alone is what scheduled a terraform plan for `fungus`
/// at `platform-dev/eu-west-1/infrastructure-platform`, an ECS place, where the
/// artifact carries no terraform to run. The destination it *did* declare was
/// released to as well, so the failure read as "half the release is broken"
/// rather than as "forest scheduled something nobody asked for".
pub fn selects(item: &DeploymentItem, destination_name: &str, destination_type: &str) -> bool {
    matches(&item.destination, destination_name)
        && type_matches(&item.destination_type, destination_type)
}

/// Narrow a set of candidate destinations to the ones a project declared for
/// `environment`.
///
/// The single implementation of the rule, shared by the pipeline path (a deploy
/// or plan stage activating) and the request path (`forest release` naming an
/// environment). They were separate once, which is how the pipeline path spent a
/// release cycle ignoring declarations the request path was already honouring.
///
/// `describe` yields a candidate's `(name, type)`. Both are consulted — see
/// [`selects`].
///
/// - Nothing declared for `environment` → every candidate, unchanged. This is the
///   compatibility hinge: a project that declares no destinations must keep
///   releasing to all of them.
/// - Declared, and something matches → the matching subset.
/// - Declared, and nothing matches → `Err` with a message naming the environment,
///   what it holds, and the selectors that matched none of it. Never an empty
///   success: silently deploying nothing is the failure this whole mechanism
///   exists to prevent.
///
/// A candidate the declaration does not cover is simply **not scheduled**. It is
/// not a failure and it does not appear on the release at all: no row, no red
/// stage, and nothing counted against "stages complete". Failing it instead
/// would be describing a project's own scoping decision as a problem with the
/// release.
///
/// A selector that matches nothing while a sibling matches something is a
/// warning, not an error — a regex is allowed not to match, and a project
/// declaring several regions should not break because one does not exist here
/// yet.
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
        return Ok(candidates);
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

    // Matched the place but not the kind. Kept separately because it is the one
    // exclusion worth explaining: "your selector found this destination, and
    // then the types disagreed" is a different thing to debug from "your
    // selector found nothing".
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

/// `name (type)`, or just the name when the caller has no type to offer.
fn describe_one<T>(describe: &impl Fn(&T) -> (&str, &str), candidate: &T) -> String {
    let (name, destination_type) = describe(candidate);
    if destination_type.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({destination_type})")
    }
}

/// The same shape for the other side of the comparison, so a failure message
/// puts `^dev/.*$ (forest/terraform@1)` next to
/// `platform-dev/… (forest/generic@1)` and the mismatch reads off the line.
fn describe_declared(item: &DeploymentItem) -> String {
    if item.destination_type.is_empty() {
        item.destination.clone()
    } else {
        format!("{} ({})", item.destination, item.destination_type)
    }
}

/// A parsed item record together with the directory it governs.
pub struct ParsedItem {
    /// The item's root: everything under it belongs to this item.
    pub root: std::path::PathBuf,
    pub item: DeploymentItem,
}

/// Every deployment item in an uploaded tree, deepest root first.
///
/// Deepest-first so attribution picks the innermost enclosing item, for the case
/// of one item's root sitting inside another's (destination `a` with type `t/u`
/// nests inside destination `a/t` with type `u/v`).
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

        // <item root>/forest/config.json -> <item root>
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

/// Which item each deployment file belongs to, as `(file_name, env, destination)`.
///
/// This is the decision that used to live in the CLI — in two copies, which is
/// how one of them kept truncating selectors after the other was fixed. Deciding
/// it here means every client gets it right, including the ones nobody upgrades:
/// the server has the whole tree, and the tree says what each file is for.
///
/// A file under no item is omitted. Its `(env, destination)` cannot be known,
/// and recording a guess is what produced the original bug.
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

/// The selectors a declaration names for one environment.
///
/// An environment the project said nothing about yields nothing, which callers
/// read as "no declaration, do not filter" — the backwards-compatible default
/// that keeps every project which declares no destinations releasing to whole
/// environments.
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
        selectors_for_env, type_matches,
    };

    /// What `release prepare` writes, at the path it writes it to — selector and
    /// destination type both carrying a `/`.
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

    /// The shape every project in the field actually writes.
    #[test]
    fn an_anchored_selector_matches_its_place_and_nothing_else() {
        let selector = "^data-prod/.*$";

        assert!(matches(
            selector,
            "data-prod/eu-west-1/infrastructure-data-ecs"
        ));

        // The destination that took the misrouted release: same environment,
        // different place. This assertion is the bug.
        assert!(!matches(selector, "data"));

        // Neighbouring environments must not be caught by a prod selector.
        assert!(!matches(selector, "data-dev/eu-west-1/infrastructure-data"));
    }

    /// Anchoring belongs to whoever wrote the selector. Documented as a test
    /// because the tempting "helpful" change here is to wrap every pattern in
    /// `^…$`, which would quietly narrow every selector already deployed.
    #[test]
    fn an_unanchored_selector_is_left_unanchored() {
        assert!(matches("data-prod", "data-prod/eu-west-1/x"));
        assert!(matches("data-prod", "legacy-data-prod-mirror"));
    }

    /// A pattern that will not compile means "this one destination", not
    /// "every destination".
    #[test]
    fn an_unparseable_pattern_falls_back_to_equality() {
        let broken = "^(data-prod";

        assert!(matches(broken, broken));
        assert!(!matches(broken, "data-prod/eu-west-1/x"));
        assert!(!matches(broken, "anything-else"));
    }

    /// `annotate` used to split the rendered path on `/` and keep one
    /// component, so artifacts uploaded before that was fixed recorded
    /// `^data-prod` where the project wrote `^data-prod/.*$`. Truncating a
    /// regex at a `/` drops a constraint, so it can only ever match *more* —
    /// which is why reading such a value is a widening, never a narrowing, and
    /// why the pipeline path was still correct for the destinations in play.
    #[test]
    fn a_truncated_selector_still_matches_what_the_full_one_would() {
        assert!(matches("^data-prod", "data-prod/eu-west-1/x"));
        assert!(!matches("^data-prod", "data"));
    }

    /// The whole point: the selector survives here intact, where the path-derived
    /// `artifact_files.destination` column records it cut at the first `/`.
    #[test]
    fn the_declaration_keeps_the_selector_whole() {
        let files = vec![item_record("data", "^data-prod/.*$")];

        let items = parse_declaration(&files).expect("parse");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].env, "data");
        assert_eq!(items[0].destination, "^data-prod/.*$");
        assert_eq!(items[0].destination_type, "forest/generic@1");
    }

    /// Manifests sit beside the item record and must not be mistaken for one.
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

    /// A project that prepared nothing declared nothing. Distinct from `NULL`,
    /// which means "annotated before this existed" — both fan out, but only one
    /// of them is a statement.
    #[test]
    fn no_item_records_is_an_empty_declaration_not_an_error() {
        let files = vec![(
            PathBuf::from("forest.cue"),
            "project: name: \"x\"\n".to_string(),
        )];

        assert!(parse_declaration(&files).expect("parse").is_empty());
    }

    /// The contract that keeps a partial declaration from ever being stored.
    /// If this test is ever "fixed" by making the parse skip bad records, the
    /// filter silently narrows and nothing reports it.
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

        // The reader needs to know which file, or they are diffing a tree by eye.
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

        // `canopy-hubspot-ingest`'s live shape: its pipeline deploys `data-prod`
        // while its forest.cue still declares `data`. Nothing declared for the
        // stage's environment means no filtering, not an empty filter.
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

    /// The slice-registry case: a destination the project did not declare gets
    /// no config. This is what `genericv1` was already doing correctly while the
    /// scheduler sent releases there anyway.
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

        // The selector would match, but it was declared for another environment.
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

    /// `Regex::new("")` is valid and matches every string, so the naive
    /// implementation turned an empty selector into "release to the whole
    /// environment" — the silent widening this module exists to stop, arriving
    /// through the one input nobody writes deliberately.
    #[test]
    fn an_empty_selector_selects_nothing_not_everything() {
        assert!(!matches("", "data-prod/eu-west-1/x"));
        assert!(!matches("", "data"));
        assert!(!matches("", ""));
    }

    /// And it should never reach the column in the first place.
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

    /// The corollary for the read side: an empty selector already sitting in the
    /// column (written before the guard existed) must not select anything either.
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

    // ── The declared-type intersection ──────────────────────────────────
    //
    // `fungus` is the worked example throughout. Its forest.cue declares two
    // items for one environment:
    //
    //     understory/service  ^dev/.*$          forest/terraform@1
    //     project             ^platform-dev/.*$ forest/generic@1
    //
    // and `platform-dev` holds two destinations, one of each type. Selecting on
    // the name alone puts every item at every destination whose name matches
    // any of them.

    /// A candidate as the scheduler describes it: name, then type.
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

    /// The bug, in one assertion. A terraform item and an ECS destination in the
    /// same environment: the selector reaches it, the type does not, so it is
    /// not scheduled.
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

        // Unanchored on purpose: this is what `fungus` shipped, and both names
        // match it. Only the type tells them apart.
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

    /// And the other direction, which is the same mistake wearing the other hat:
    /// an ECS item must not schedule a terraform plan.
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

    /// `fungus` as it stands today: both types declared, both destinations
    /// reached. The filter is an intersection, not a veto — over-filtering would
    /// break the very release it is meant to fix.
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

    /// A destination excluded only by its type is *out of scope*, not failed. It
    /// leaves no release row, so it cannot show up red on the release view and
    /// cannot be counted against the stages that are in scope — which is the
    /// difference between `0/2 complete` with a phantom terraform row and `1/1`.
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

    /// When the *only* thing wrong is the type, the message has to say so.
    /// "matches none of them" next to a destination the selector obviously
    /// matches is the kind of error that sends someone to rewrite a working
    /// regex.
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

    /// The compatibility hinge. An artifact annotated before item records
    /// carried a type has `destination_type: ""`, which is not a claim about
    /// kind — filtering on it would fail stages for projects that never said
    /// anything wrong.
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

    /// And a caller with no type to offer is not filtered either.
    #[test]
    fn a_candidate_with_no_known_type_is_not_filtered_out() {
        assert!(type_matches("", "forest/generic@1"));
        assert!(type_matches("forest/generic@1", ""));
        assert!(type_matches("", ""));
    }

    /// Types compare as the whole `organisation/name@version` string. `@1` and
    /// `@2` of one type are different destination types, and forest already
    /// treats them that way everywhere else.
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

    /// Declaring nothing at all for an environment still fans out — the hinge
    /// every project that names no destinations depends on. Unchanged by the
    /// type filter, and asserted here because the filter now runs over items
    /// rather than over selectors.
    #[test]
    fn nothing_declared_for_the_environment_still_fans_out() {
        let items = vec![declared("some-other-env", "^dev/.*$", "forest/terraform@1")];

        let candidates = vec![
            dest("a", "forest/generic@1"),
            dest("b", "forest/terraform@1"),
        ];

        let selected =
            narrow_to_declared(candidates, &items, "platform-dev", describe).expect("no filter");

        assert_eq!(selected.len(), 2);
    }

    /// `selects` is the single rule; `config_for_destination` uses it too, so a
    /// destination cannot be scheduled by one item and configured by another.
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

        // Both selectors match this name; only the type separates them, and the
        // terraform item is first in the list.
        let config = config_for_destination(
            &items,
            "platform-dev",
            "platform-dev/eu-west-1/infrastructure-platform",
            "forest/generic@1",
        );

        assert_eq!(config.get("service").map(String::as_str), Some("ecs_one"));
    }

    /// End to end from the real thing: the item records `forest release prepare`
    /// writes for `fungus`, at the paths it writes them to, against the two
    /// destinations `understory`'s `platform-dev` actually holds.
    ///
    /// Captured from the repo rather than invented, because the two details that
    /// matter are easy to get wrong from memory: the selector carries a `/`, and
    /// the two items differ only in their type.
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

        // And each one gets its own item's config, not the other's.
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

    /// And the version of that declaration this fixes: before `fungus` anchored
    /// its selector, the terraform item read `dev/.*`, which finds "dev" inside
    /// "platform-dev". Anchoring was the workaround; consulting the type is what
    /// makes the workaround unnecessary.
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

        // Only the generic item can claim it, and that is the one whose config
        // and manifests belong there.
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
