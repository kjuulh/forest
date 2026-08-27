use std::sync::Arc;

use crate::{
    State,
    destinations::{
        DestinationIndex, DestinationService,
        external::{self, ExternalDestination},
    },
    services::release_logs_registry::ReleaseLogsRegistryState,
};

#[derive(Clone)]
pub struct DestinationServices {
    services: Arc<Vec<DestinationService>>,
}

impl DestinationServices {
    /// Returns lightweight identity records used for internal lookups.
    pub fn list_indexes(&self) -> Vec<DestinationIndex> {
        self.services.iter().map(|s| s.name()).collect()
    }

    /// Returns the full domain model for every registered destination type,
    /// including description and metadata field schemas. Used by the gRPC
    /// `ListDestinationTypes` handler.
    pub fn list_types(&self) -> Vec<forest_models::DestinationType> {
        self.services
            .iter()
            .map(|s| {
                let idx = s.name();
                forest_models::DestinationType {
                    organisation: idx.organisation,
                    name: idx.name,
                    version: idx.version,
                    description: s.description().to_owned(),
                    fields: s.metadata_schema(),
                }
            })
            .collect()
    }

    pub fn get_destination(
        &self,
        organisation: &str,
        name: &str,
        version: usize,
    ) -> Option<&DestinationService> {
        let index = DestinationIndex {
            organisation: organisation.into(),
            name: name.into(),
            version,
        };
        self.services.iter().find(|i| i.name() == index)
    }
}

pub trait DestinationServicesState {
    fn destination_services(&self) -> DestinationServices;
}

impl DestinationServicesState for State {
    fn destination_services(&self) -> DestinationServices {
        let release_logs_registry = self.release_logs_registry();
        let mut services = vec![
            DestinationService::new_flux_v1(self, release_logs_registry.clone()),
            DestinationService::new_kubernetes_v1(release_logs_registry.clone()),
            DestinationService::new_forage_v1(release_logs_registry.clone()),
            DestinationService::new_terraform_v1(self, release_logs_registry.clone()),
        ];

        // Types implemented by a runner rather than by forest. They carry a
        // metadata schema so destinations can be created and validated, but no
        // implementation — the scheduler dispatches them to whichever runner
        // registers the matching capability.
        for spec in external::declared() {
            if services.iter().any(|s| s.name() == spec.index()) {
                // A built-in of the same name wins; overriding a compiled-in
                // implementation from a config file is not something a
                // deployment should be able to do by accident.
                tracing::warn!(
                    destination_type = %spec.index(),
                    "ignoring externally-declared destination type: a built-in of the same name exists",
                );
                continue;
            }

            services.push(DestinationService::new(
                ExternalDestination { spec: spec.clone() },
                release_logs_registry.clone(),
            ));
        }

        DestinationServices {
            services: Arc::new(services),
        }
    }
}
