use anyhow::Context;
use forest_grpc_interface::{destination_service_server::DestinationService, *};
use tonic::Response;

use crate::{
    destination_services::DestinationServicesState,
    grpc::artifacts::GrpcErrorExt,
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
        let req = request.into_inner();

        tracing::debug!("create destination: {:?}", req);

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
                &dest_type.organisation,
                &dest_type.name,
                dest_type.version as u32,
            )
            .await
            .context("create destination")
            .to_internal_error()?;

        self.state.event_bus().emit(EventPayload {
            organisation: req.organisation.clone(),
            project: String::new(),
            resource_type: "destination",
            action: "created",
            resource_id: req.name.clone(),
            metadata: [("environment".into(), req.environment.clone())].into(),
        }).await;

        Ok(Response::new(CreateDestinationResponse {}))
    }

    async fn update_destination(
        &self,
        request: tonic::Request<UpdateDestinationRequest>,
    ) -> std::result::Result<tonic::Response<UpdateDestinationResponse>, tonic::Status> {
        let req = request.into_inner();

        self.state
            .destination_aggregate_service()
            .update_metadata(&req.name, req.metadata)
            .await
            .context("update destination")
            .to_internal_error()?;

        self.state.event_bus().emit(EventPayload {
            organisation: String::new(),
            project: String::new(),
            resource_type: "destination",
            action: "updated",
            resource_id: req.name.clone(),
            metadata: Default::default(),
        }).await;

        Ok(Response::new(UpdateDestinationResponse {}))
    }

    async fn delete_destination(
        &self,
        request: tonic::Request<DeleteDestinationRequest>,
    ) -> std::result::Result<tonic::Response<DeleteDestinationResponse>, tonic::Status> {
        let req = request.into_inner();

        self.state
            .destination_aggregate_service()
            .delete_destination(&req.name)
            .await
            .context("delete destination")
            .to_internal_error()?;

        self.state.event_bus().emit(EventPayload {
            organisation: String::new(),
            project: String::new(),
            resource_type: "destination",
            action: "deleted",
            resource_id: req.name.clone(),
            metadata: Default::default(),
        }).await;

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
            .map(|idx| DestinationType {
                organisation: idx.organisation,
                name: idx.name,
                version: idx.version as u64,
            })
            .collect();

        Ok(Response::new(ListDestinationTypesResponse { types }))
    }

    async fn get_destinations(
        &self,
        request: tonic::Request<GetDestinationsRequest>,
    ) -> std::result::Result<tonic::Response<GetDestinationsResponse>, tonic::Status> {
        tracing::debug!("get destinations");
        let req = request.into_inner();

        let destinations = self
            .state
            .release_registry()
            .get_destinations(&req.organisation)
            .await
            .context("failed to find destinations")
            .to_internal_error()?;

        Ok(Response::new(GetDestinationsResponse {
            destinations: destinations.into_iter().map(|n| n.into()).collect(),
        }))
    }
}
