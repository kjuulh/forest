use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::release_registry::ReleaseItem,
};

/// A destination that deploys nothing, on purpose (DATA-637).
///
/// Forest's release lifecycle is what people actually consume: a release goes
/// QUEUED → RUNNING → SUCCEEDED, and each transition fires a notification that
/// forage fans out to Slack. Services whose rollout forest does *not* own —
/// commission-dashboard builds an image in GitHub Actions and force-new-deploys
/// an ECS service — still want that, and today they cannot have it: a release
/// needs at least one destination, and every other destination type tries to
/// deploy something.
///
/// So this one succeeds without touching anything. It carries no credentials,
/// requires no metadata, and reads no artifacts, which is what makes it safe to
/// point at a service forest has no access to. Adopting it is a statement:
/// *forest announces this, something else ships it*.
///
/// Two things it deliberately is not:
///
///   - Not a placeholder for a real destination. `forest/kubernetes@1` is a stub
///     whose `release` also returns `Ok(())`, and reaching for that would mean a
///     future implementation silently starts deploying to a cluster nobody
///     configured. A type that promises nothing cannot break that promise.
///   - Not a health signal. There is nothing to be healthy, so a release here
///     says "the annotation is recorded and the announcement went out", not "the
///     service is up". Whatever actually deployed still owns that.
pub struct NoopV1Destination {}

#[async_trait::async_trait]
impl DestinationEdge for NoopV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "noop".into(),
            version: 1,
        }
    }

    fn description(&self) -> &str {
        "Deploy nothing. Records the release and fires its notifications, for services deployed outside forest."
    }

    /// No fields — the point of this type is that it needs no access to
    /// anything. A destination whose form is empty is a hint in itself.
    fn metadata_schema(&self) -> Vec<forest_models::MetadataFieldSchema> {
        vec![]
    }

    /// The log block is the only artefact of the release, so it says what
    /// happened and — more usefully when someone is staring at a green release
    /// wondering why nothing moved — what did not.
    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        logger.log_stdout(&format!(
            "forest/noop: destination '{}' in environment '{}' deploys nothing by design.",
            destination.name, destination.environment
        ));
        logger.log_stdout(&format!(
            "forest/noop: recorded release for project '{}' — notifications fire as usual.",
            release.project
        ));
        logger.log_stdout(
            "forest/noop: whatever actually deployed this service did so outside forest.",
        );

        tracing::info!(
            release_id = %release.id,
            destination = %destination.name,
            environment = %destination.environment,
            project = %release.project,
            "noop destination: nothing to deploy, reporting success"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn it_is_addressable_as_forest_noop_v1() {
        assert_eq!(
            NoopV1Destination {}.name(),
            DestinationIndex {
                organisation: "forest".into(),
                name: "noop".into(),
                version: 1,
            }
        );
    }

    /// The whole contract: no required metadata, so a destination of this type
    /// can be created without handing forest a single credential.
    #[test]
    fn it_requires_no_metadata() {
        let dest = NoopV1Destination {};

        assert!(dest.metadata_schema().is_empty());
        assert!(dest.validate_metadata(&HashMap::new()).is_ok());
    }

    /// And it declines the plan phase rather than reporting an empty plan, which
    /// would read as "no changes" from a destination that never has any.
    #[test]
    fn it_does_not_support_plan() {
        assert!(!NoopV1Destination {}.supports_plan());
    }
}
