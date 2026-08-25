use anyhow::Context;
use forest_grpc_interface::{destination_service_server::DestinationService, *};
use tonic::Response;

use crate::{
    destination_services::DestinationServicesState,
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        destination_aggregate::DestinationAggregateServiceState,
        event_bus::{EventBusState, EventPayload},
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct DestinationServer {
    pub state: State,
}

#[async_trait::async_trait]
impl DestinationService for DestinationServer {
    async fn create_destination(
        &self,
        request: tonic::Request<CreateDestinationRequest>,
    ) -> std::result::Result<tonic::Response<CreateDestinationResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        // Never log the request wholesale: `req.metadata` carries credentials.
        // Key names are enough to debug a create.
        tracing::debug!(
            organisation = %req.organisation,
            name = %req.name,
            environment = %req.environment,
            metadata_keys = ?req.metadata.keys().collect::<Vec<_>>(),
            sensitive_keys = ?req.sensitive_keys,
            "create destination"
        );

        let dest_type: forest_models::DestinationType = req
            .r#type
            .context("destination type is required")
            .to_internal_error()?
            .into();

        let dest_services = self.state.destination_services();
        let dest_svc = dest_services
            .get_destination(&dest_type.organisation, &dest_type.name, dest_type.version)
            .context("failed to find destination implementation")
            .to_internal_error()?;

        dest_svc
            .validate_metadata(&req.metadata)
            .context("invalid destination metadata")
            .to_internal_error()?;

        self.state
            .destination_aggregate_service()
            .create_destination(
                &req.organisation,
                &req.name,
                &req.environment,
                req.metadata,
                req.sensitive_keys,
                &dest_type.organisation,
                &dest_type.name,
                dest_type.version as u32,
            )
            .await
            .context("create destination")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: req.organisation.clone(),
                project: String::new(),
                resource_type: "destination",
                action: "created",
                resource_id: req.name.clone(),
                metadata: [("environment".into(), req.environment.clone())].into(),
            })
            .await;

        Ok(Response::new(CreateDestinationResponse {}))
    }

    async fn update_destination(
        &self,
        request: tonic::Request<UpdateDestinationRequest>,
    ) -> std::result::Result<tonic::Response<UpdateDestinationResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        if req.organisation.is_empty() {
            return Err(tonic::Status::invalid_argument("organisation is required"));
        }
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        // Merging an empty overlay changes nothing, so skip the write rather
        // than record an event that says the metadata stayed the same. This is
        // the shape a caller sends when it only means to touch sensitive_keys.
        if !(req.merge_metadata && req.metadata.is_empty()) {
            self.state
                .destination_aggregate_service()
                .update_metadata(
                    &req.organisation,
                    &req.name,
                    req.metadata,
                    req.merge_metadata,
                )
                .await
                .context("update destination")
                .to_internal_error()?;
        }

        // Guarded by an explicit flag so older clients, which cannot send the
        // field, don't silently clear an existing set.
        if req.set_sensitive_keys {
            self.state
                .destination_aggregate_service()
                .update_sensitive_keys(&req.organisation, &req.name, req.sensitive_keys)
                .await
                .context("update destination sensitive keys")
                .to_internal_error()?;
        }

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: req.organisation.clone(),
                project: String::new(),
                resource_type: "destination",
                action: "updated",
                resource_id: req.name.clone(),
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(UpdateDestinationResponse {}))
    }

    async fn delete_destination(
        &self,
        request: tonic::Request<DeleteDestinationRequest>,
    ) -> std::result::Result<tonic::Response<DeleteDestinationResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        if req.organisation.is_empty() {
            return Err(tonic::Status::invalid_argument("organisation is required"));
        }
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        self.state
            .destination_aggregate_service()
            .delete_destination(&req.organisation, &req.name)
            .await
            .context("delete destination")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: req.organisation.clone(),
                project: String::new(),
                resource_type: "destination",
                action: "deleted",
                resource_id: req.name.clone(),
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(DeleteDestinationResponse {}))
    }

    async fn list_destination_types(
        &self,
        _request: tonic::Request<ListDestinationTypesRequest>,
    ) -> std::result::Result<tonic::Response<ListDestinationTypesResponse>, tonic::Status> {
        let dest_services = self.state.destination_services();
        let types = dest_services
            .list_types()
            .into_iter()
            .map(Into::into)
            .collect();

        Ok(Response::new(ListDestinationTypesResponse { types }))
    }

    async fn reveal_destination_metadata(
        &self,
        request: tonic::Request<RevealDestinationMetadataRequest>,
    ) -> std::result::Result<tonic::Response<RevealDestinationMetadataResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        if req.organisation.is_empty() {
            return Err(tonic::Status::invalid_argument("organisation is required"));
        }
        if req.key.is_empty() {
            return Err(tonic::Status::invalid_argument("key is required"));
        }
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let record = self
            .state
            .destination_aggregate_service()
            .get_by_name(&req.organisation, &req.name)
            .await
            .context("get destination")
            .to_internal_error()?
            .ok_or_else(|| tonic::Status::not_found("destination not found"))?;

        let value = record.metadata.get(&req.key).ok_or_else(|| {
            tonic::Status::not_found(format!("destination has no metadata key '{}'", req.key))
        })?;

        // One key per call, and the key name is recorded — pulling a
        // credential leaves a trail. The value never enters the audit event.
        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: req.organisation.clone(),
                project: String::new(),
                resource_type: "destination",
                action: "metadata_revealed",
                resource_id: req.name.clone(),
                metadata: [("key".into(), req.key.clone())].into(),
            })
            .await;

        tracing::info!(
            organisation = %req.organisation,
            destination = %req.name,
            key = %req.key,
            "destination metadata revealed"
        );

        Ok(Response::new(RevealDestinationMetadataResponse {
            key: req.key,
            value: value.clone(),
        }))
    }

    async fn get_destinations(
        &self,
        request: tonic::Request<GetDestinationsRequest>,
    ) -> std::result::Result<tonic::Response<GetDestinationsResponse>, tonic::Status> {
        tracing::debug!("get destinations");
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let mut destinations = self
            .state
            .release_registry()
            .get_destinations(&req.organisation)
            .await
            .context("failed to find destinations")
            .to_internal_error()?;

        // The projection stores only the type's coordinates, so join the live
        // type registry to recover each field schema. Without this the
        // `sensitive` flags are invisible here and type-declared credentials
        // would be serialised in full.
        let dest_services = self.state.destination_services();
        for dest in &mut destinations {
            let t = &dest.destination_type;
            if let Some(svc) = dest_services.get_destination(&t.organisation, &t.name, t.version) {
                dest.destination_type.fields = svc.metadata_schema();
            }
        }

        // The `Destination -> proto` conversion is what drops sensitive values;
        // see `forest_models::Destination::partition_metadata`.
        Ok(Response::new(GetDestinationsResponse {
            destinations: destinations.into_iter().map(|n| n.into()).collect(),
        }))
    }
}
