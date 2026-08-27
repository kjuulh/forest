//! Externally-declared destination types.
//!
//! Most destination types are part of forest: `flux`, `kubernetes`, `terraform`,
//! `forage` all have an in-process implementation compiled in. Some are not, and
//! should not be — a type that talks to one vendor's API, needs its own identity,
//! or is only wanted in one deployment has no business being in every forest
//! build.
//!
//! `RegisterRunner` already routes work purely by declared
//! `DestinationCapability`, so a runner-only type needs nothing from the server
//! at dispatch time. Creation is the gap: `CreateDestination` looks the type up
//! in `DestinationServices` and refuses anything it cannot find, so a type with
//! no in-process implementation cannot be created at all.
//!
//! This closes that gap generically. A deployment declares the types its runners
//! serve in a JSON file, and forest gains the metadata schema and validation for
//! them without gaining an implementation — or any knowledge of what they do.
//!
//! `FOREST_EXTERNAL_DESTINATION_TYPES` holds either the JSON itself or a path to
//! a file containing it:
//!
//! ```jsonc
//! [
//!   {
//!     "organisation": "understory",
//!     "name": "ecs",
//!     "version": 1,
//!     "description": "Release an existing ECS service in a target AWS account.",
//!     "supports_plan": true,
//!     "fields": [
//!       { "name": "cluster", "label": "ECS Cluster", "required": true, "field_type": "text" }
//!     ]
//!   }
//! ]
//! ```

use std::{collections::HashMap, sync::OnceLock};

use anyhow::Context;
use forest_models::Destination;
use serde::Deserialize;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::release_registry::ReleaseItem,
};

/// Externally-implemented destination types: either the JSON itself, or a path
/// to a file containing it.
pub const EXTERNAL_TYPES_ENV: &str = "FOREST_EXTERNAL_DESTINATION_TYPES";

/// A destination type implemented by a runner rather than by forest.
#[derive(Debug, Clone, Deserialize)]
pub struct ExternalDestinationType {
    pub organisation: String,
    pub name: String,
    pub version: usize,
    #[serde(default)]
    pub description: String,
    /// Whether the runner implements a plan phase. Plans run on the runner, so
    /// forest only needs to know whether to offer one.
    #[serde(default = "default_supports_plan")]
    pub supports_plan: bool,
    #[serde(default)]
    pub fields: Vec<ExternalMetadataField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalMetadataField {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default = "default_field_type")]
    pub field_type: String,
    #[serde(default)]
    pub default_value: String,
}

fn default_supports_plan() -> bool {
    true
}

fn default_field_type() -> String {
    "text".to_string()
}

impl ExternalDestinationType {
    pub fn index(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: self.organisation.clone(),
            name: self.name.clone(),
            version: self.version,
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.organisation.trim().is_empty() || self.name.trim().is_empty() {
            anyhow::bail!("external destination type needs a non-empty organisation and name");
        }
        if self.version == 0 {
            anyhow::bail!(
                "external destination type {}/{} needs a version greater than zero",
                self.organisation,
                self.name,
            );
        }
        for field in &self.fields {
            if field.name.trim().is_empty() {
                anyhow::bail!("external destination type {} has a field with no name", self.index());
            }
        }
        Ok(())
    }
}

static DECLARED: OnceLock<Vec<ExternalDestinationType>> = OnceLock::new();

/// Read and validate the declarations, caching them for [`declared`].
///
/// Called at startup so a malformed file fails the server loudly. Silently
/// serving an empty list would leave every external destination un-creatable
/// with no indication why.
pub fn load() -> anyhow::Result<usize> {
    let types = read_from_env()?;
    let count = types.len();

    for t in &types {
        tracing::info!(
            destination_type = %t.index(),
            "registered externally-implemented destination type",
        );
    }

    // `set` fails only if something already read the declarations; that value
    // came from the same source, so there is nothing to reconcile.
    let _ = DECLARED.set(types);

    Ok(count)
}

/// Cached declarations, loading them on first use if [`load`] has not run.
pub fn declared() -> &'static [ExternalDestinationType] {
    DECLARED
        .get_or_init(|| match read_from_env() {
            Ok(types) => types,
            Err(e) => {
                tracing::error!(
                    "failed to load external destination types from {EXTERNAL_TYPES_ENV}: {e:#}"
                );
                Vec::new()
            }
        })
        .as_slice()
}

fn read_from_env() -> anyhow::Result<Vec<ExternalDestinationType>> {
    let Ok(value) = std::env::var(EXTERNAL_TYPES_ENV) else {
        return Ok(Vec::new());
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }

    // Inline JSON or a path. ECS task definitions carry environment variables,
    // not files, so requiring a path would mean baking the declaration into the
    // forest image — which would defeat the point of declaring it per deployment.
    if value.starts_with('[') {
        return parse(value).with_context(|| format!("failed to parse inline {EXTERNAL_TYPES_ENV}"));
    }

    let contents = std::fs::read_to_string(value)
        .with_context(|| format!("failed to read {EXTERNAL_TYPES_ENV} file at {value}"))?;

    parse(&contents).with_context(|| format!("failed to parse {value}"))
}

/// Parse and validate a declaration file.
pub fn parse(contents: &str) -> anyhow::Result<Vec<ExternalDestinationType>> {
    let types: Vec<ExternalDestinationType> =
        serde_json::from_str(contents).context("external destination types must be a JSON array")?;

    for t in &types {
        t.validate()?;
    }

    for (i, t) in types.iter().enumerate() {
        if types.iter().skip(i + 1).any(|other| other.index() == t.index()) {
            anyhow::bail!("external destination type {} is declared more than once", t.index());
        }
    }

    Ok(types)
}

/// A destination type forest knows the shape of but does not implement.
///
/// It carries the metadata schema so destinations can be created and validated,
/// and nothing else. Execution belongs to whichever runner declares the matching
/// capability; the scheduler tries runners before falling back in-process, so
/// reaching [`DestinationEdge::release`] here means no such runner was connected.
pub struct ExternalDestination {
    pub spec: ExternalDestinationType,
}

impl ExternalDestination {
    fn no_runner(&self) -> anyhow::Error {
        anyhow::anyhow!(
            "destination type {} is implemented externally and can only run on a runner that \
             registers the {}/{}@{} capability. No such runner is currently connected, so this \
             release cannot be executed.",
            self.spec.index(),
            self.spec.organisation,
            self.spec.name,
            self.spec.version,
        )
    }
}

#[async_trait::async_trait]
impl DestinationEdge for ExternalDestination {
    fn name(&self) -> DestinationIndex {
        self.spec.index()
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn metadata_schema(&self) -> Vec<forest_models::MetadataFieldSchema> {
        self.spec
            .fields
            .iter()
            .map(|f| forest_models::MetadataFieldSchema {
                name: f.name.clone(),
                label: if f.label.is_empty() {
                    f.name.clone()
                } else {
                    f.label.clone()
                },
                description: f.description.clone(),
                required: f.required,
                field_type: f.field_type.clone(),
                default_value: f.default_value.clone(),
            })
            .collect()
    }

    /// Only the declared schema can be checked here — the runner does the
    /// type-specific validation when it runs. Missing required fields are worth
    /// catching at creation time regardless, since they are the common mistake.
    fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        let missing: Vec<_> = self
            .spec
            .fields
            .iter()
            .filter(|f| f.required)
            .filter(|f| {
                metadata
                    .get(&f.name)
                    .map(|v| v.trim().is_empty())
                    .unwrap_or(true)
            })
            .map(|f| f.name.as_str())
            .collect();

        if !missing.is_empty() {
            anyhow::bail!(
                "destination type {} requires metadata: {}",
                self.spec.index(),
                missing.join(", "),
            );
        }

        Ok(())
    }

    async fn prepare(
        &self,
        _logger: &DestinationLogger,
        _release: &ReleaseItem,
        _destination: &Destination,
    ) -> anyhow::Result<()> {
        Err(self.no_runner())
    }

    async fn release(
        &self,
        _logger: &DestinationLogger,
        _release: &ReleaseItem,
        _destination: &Destination,
    ) -> anyhow::Result<()> {
        Err(self.no_runner())
    }

    async fn plan(
        &self,
        _logger: &DestinationLogger,
        _release: &ReleaseItem,
        _destination: &Destination,
    ) -> anyhow::Result<Option<String>> {
        Err(self.no_runner())
    }

    fn supports_plan(&self) -> bool {
        self.spec.supports_plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ECS: &str = r#"[
      {
        "organisation": "understory",
        "name": "ecs",
        "version": 1,
        "description": "Release an existing ECS service.",
        "supports_plan": true,
        "fields": [
          { "name": "cluster", "label": "ECS Cluster", "required": true, "field_type": "text" },
          { "name": "role_arn", "required": false }
        ]
      }
    ]"#;

    fn ecs() -> ExternalDestination {
        ExternalDestination {
            spec: parse(ECS).unwrap().remove(0),
        }
    }

    #[test]
    fn parses_a_declaration() {
        let types = parse(ECS).unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].index().to_string(), "understory/ecs@1");
        assert!(types[0].supports_plan);
    }

    #[test]
    fn an_empty_declaration_file_is_fine() {
        assert!(parse("[]").unwrap().is_empty());
    }

    #[test]
    fn plan_support_defaults_to_on() {
        let types =
            parse(r#"[{"organisation":"o","name":"n","version":1}]"#).unwrap();
        assert!(types[0].supports_plan);
    }

    #[test]
    fn duplicate_declarations_are_rejected() {
        let json = r#"[
          {"organisation":"o","name":"n","version":1},
          {"organisation":"o","name":"n","version":1}
        ]"#;
        let err = parse(json).unwrap_err().to_string();
        assert!(err.contains("more than once"), "got: {err}");
    }

    #[test]
    fn a_type_without_a_version_is_rejected() {
        let err = parse(r#"[{"organisation":"o","name":"n","version":0}]"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("version"), "got: {err}");
    }

    #[test]
    fn a_type_without_a_name_is_rejected() {
        let err = parse(r#"[{"organisation":"o","name":"","version":1}]"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("non-empty"), "got: {err}");
    }

    #[test]
    fn a_declaration_is_recognised_inline_or_by_path() {
        // Inline is the ECS case: task definitions carry environment variables,
        // not files.
        unsafe { std::env::set_var(EXTERNAL_TYPES_ENV, ECS) };
        assert_eq!(read_from_env().unwrap().len(), 1);

        let file = std::env::temp_dir().join("forest-external-types-test.json");
        std::fs::write(&file, ECS).unwrap();
        unsafe { std::env::set_var(EXTERNAL_TYPES_ENV, &file) };
        assert_eq!(read_from_env().unwrap().len(), 1);

        // An unset or blank value means "no external types", not an error.
        unsafe { std::env::set_var(EXTERNAL_TYPES_ENV, "  ") };
        assert!(read_from_env().unwrap().is_empty());

        // A path that isn't there is an error, not a silent empty list.
        unsafe { std::env::set_var(EXTERNAL_TYPES_ENV, "/nope/does-not-exist.json") };
        assert!(read_from_env().is_err());

        unsafe { std::env::remove_var(EXTERNAL_TYPES_ENV) };
        assert!(read_from_env().unwrap().is_empty());

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn malformed_json_is_rejected_rather_than_ignored() {
        assert!(parse("not json").is_err());
        // An object rather than an array is the likely hand-editing mistake.
        assert!(parse(r#"{"organisation":"o","name":"n","version":1}"#).is_err());
    }

    #[test]
    fn schema_fills_in_a_missing_label_from_the_field_name() {
        let schema = ecs().metadata_schema();
        assert_eq!(schema[0].label, "ECS Cluster");
        assert_eq!(schema[1].label, "role_arn");
        assert_eq!(schema[1].field_type, "text");
    }

    #[test]
    fn required_metadata_is_enforced_at_creation() {
        let ecs = ecs();

        let err = ecs
            .validate_metadata(&HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("cluster"), "got: {err}");

        // Present but blank is the same as absent.
        let blank = HashMap::from([("cluster".to_string(), "   ".to_string())]);
        assert!(ecs.validate_metadata(&blank).is_err());

        let ok = HashMap::from([("cluster".to_string(), "infrastructure-platform".to_string())]);
        ecs.validate_metadata(&ok).unwrap();
    }

    #[tokio::test]
    async fn executing_in_process_explains_that_a_runner_is_required() {
        let err = ecs().no_runner().to_string();
        assert!(err.contains("understory/ecs@1"), "got: {err}");
        assert!(err.contains("runner"), "got: {err}");
    }
}
