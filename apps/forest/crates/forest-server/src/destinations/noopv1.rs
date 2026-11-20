use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::release_registry::ReleaseItem,
};

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

    fn metadata_schema(&self) -> Vec<forest_models::MetadataFieldSchema> {
        vec![]
    }

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

    #[tokio::test]
    async fn it_requires_no_metadata() {
        let dest = NoopV1Destination {};

        assert!(dest.metadata_schema().is_empty());
        assert!(dest.validate_metadata(&HashMap::new()).await.is_ok());
    }

    #[test]
    fn it_does_not_support_plan() {
        assert!(!NoopV1Destination {}.supports_plan());
    }
}
