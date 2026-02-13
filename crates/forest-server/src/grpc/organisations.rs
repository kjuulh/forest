use forest_grpc_interface::{organisation_service_server::OrganisationService, *};
use uuid::Uuid;

use crate::{services::organisations::OrganisationServiceState, state::State, tokens::AppClaims};

pub struct OrganisationsServer {
    pub state: State,
}

#[async_trait::async_trait]
impl OrganisationService for OrganisationsServer {
    async fn create_organisation(
        &self,
        request: tonic::Request<CreateOrganisationRequest>,
    ) -> std::result::Result<tonic::Response<CreateOrganisationResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<AppClaims>()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth context"))?;

        let creator_id = claims
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::internal("invalid user_id in token"))?;

        let req = request.into_inner();

        let created = self
            .state
            .organisation_service()
            .create_organisation(&req.name, creator_id)
            .await
            .inspect_err(|e| tracing::warn!("failed to create organisation: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(CreateOrganisationResponse {
            organisation_id: created.organisation_id.to_string(),
        }))
    }

    async fn get_organisation(
        &self,
        request: tonic::Request<GetOrganisationRequest>,
    ) -> std::result::Result<tonic::Response<GetOrganisationResponse>, tonic::Status> {
        let req = request.into_inner();

        let service = self.state.organisation_service();

        let org = match req.identifier {
            Some(get_organisation_request::Identifier::OrganisationId(id)) => {
                let uuid = id
                    .parse::<Uuid>()
                    .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;
                service.get_organisation_by_id(uuid).await
            }
            Some(get_organisation_request::Identifier::Name(name)) => {
                service.get_organisation_by_name(&name).await
            }
            None => return Err(tonic::Status::invalid_argument("identifier is required")),
        }
        .inspect_err(|e| tracing::warn!("failed to get organisation: {e:#}"))
        .map_err(|e| tonic::Status::internal(e.to_string()))?
        .ok_or_else(|| tonic::Status::not_found("organisation not found"))?;

        Ok(tonic::Response::new(GetOrganisationResponse {
            organisation: Some(org_to_grpc(org)),
        }))
    }

    async fn search_organisations(
        &self,
        request: tonic::Request<SearchOrganisationsRequest>,
    ) -> std::result::Result<tonic::Response<SearchOrganisationsResponse>, tonic::Status> {
        let req = request.into_inner();
        let page_size = if req.page_size > 0 {
            req.page_size as i64
        } else {
            50
        };
        let offset = req.page_token.parse::<i64>().unwrap_or(0);

        let result = self
            .state
            .organisation_service()
            .search_organisations(&req.query, page_size, offset)
            .await
            .inspect_err(|e| tracing::warn!("failed to search organisations: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let next_offset = offset + page_size;
        let next_page_token = if next_offset < result.total_count {
            next_offset.to_string()
        } else {
            String::new()
        };

        Ok(tonic::Response::new(SearchOrganisationsResponse {
            organisations: result.organisations.into_iter().map(org_to_grpc).collect(),
            next_page_token,
            total_count: result.total_count as i32,
        }))
    }
}

fn org_to_grpc(
    org: crate::services::organisations::OrganisationInfo,
) -> Organisation {
    Organisation {
        organisation_id: org.organisation_id.to_string(),
        name: org.name,
        created_at: Some(prost_types::Timestamp {
            seconds: org.created_at.timestamp(),
            nanos: org.created_at.timestamp_subsec_nanos() as i32,
        }),
    }
}
