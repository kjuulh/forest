use forest_grpc_interface::{organisation_service_server::OrganisationService, *};
use uuid::Uuid;

use super::error;
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
            .map_err(error::to_status)?;

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
        .map_err(error::to_status)?
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
            .map_err(error::to_status)?;

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

    async fn list_my_organisations(
        &self,
        request: tonic::Request<ListMyOrganisationsRequest>,
    ) -> std::result::Result<tonic::Response<ListMyOrganisationsResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<AppClaims>()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth context"))?;

        let user_id = claims
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::internal("invalid user_id in token"))?;

        let req = request.into_inner();
        let role_filter = if req.role.is_empty() {
            None
        } else {
            Some(req.role.as_str())
        };

        let orgs = self
            .state
            .organisation_service()
            .list_my_organisations(user_id, role_filter)
            .await
            .map_err(error::to_status)?;

        let roles: Vec<String> = orgs.iter().map(|o| o.role.clone()).collect();
        let organisations: Vec<Organisation> = orgs.into_iter().map(|o| Organisation {
            organisation_id: o.organisation_id.to_string(),
            name: o.name,
            created_at: Some(prost_types::Timestamp {
                seconds: o.created_at.timestamp(),
                nanos: o.created_at.timestamp_subsec_nanos() as i32,
            }),
        }).collect();

        Ok(tonic::Response::new(ListMyOrganisationsResponse {
            organisations,
            roles,
        }))
    }

    // -- Member management --------------------------------------------------------

    async fn add_member(
        &self,
        request: tonic::Request<AddMemberRequest>,
    ) -> std::result::Result<tonic::Response<AddMemberResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<AppClaims>()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth context"))?;

        let requester_id = claims
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::internal("invalid user_id in token"))?;

        let req = request.into_inner();

        let organisation_id = req
            .organisation_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;

        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        let member = self
            .state
            .organisation_service()
            .add_member(organisation_id, user_id, &req.role, requester_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(AddMemberResponse {
            member: Some(member_to_grpc(member)),
        }))
    }

    async fn remove_member(
        &self,
        request: tonic::Request<RemoveMemberRequest>,
    ) -> std::result::Result<tonic::Response<RemoveMemberResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<AppClaims>()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth context"))?;

        let requester_id = claims
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::internal("invalid user_id in token"))?;

        let req = request.into_inner();

        let organisation_id = req
            .organisation_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;

        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        self.state
            .organisation_service()
            .remove_member(organisation_id, user_id, requester_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(RemoveMemberResponse {}))
    }

    async fn update_member_role(
        &self,
        request: tonic::Request<UpdateMemberRoleRequest>,
    ) -> std::result::Result<tonic::Response<UpdateMemberRoleResponse>, tonic::Status> {
        let claims = request
            .extensions()
            .get::<AppClaims>()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth context"))?;

        let requester_id = claims
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::internal("invalid user_id in token"))?;

        let req = request.into_inner();

        let organisation_id = req
            .organisation_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;

        let user_id = req
            .user_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid user_id"))?;

        let member = self
            .state
            .organisation_service()
            .update_member_role(organisation_id, user_id, &req.role, requester_id)
            .await
            .map_err(error::to_status)?;

        Ok(tonic::Response::new(UpdateMemberRoleResponse {
            member: Some(member_to_grpc(member)),
        }))
    }

    async fn list_members(
        &self,
        request: tonic::Request<ListMembersRequest>,
    ) -> std::result::Result<tonic::Response<ListMembersResponse>, tonic::Status> {
        let _claims = request
            .extensions()
            .get::<AppClaims>()
            .ok_or_else(|| tonic::Status::unauthenticated("missing auth context"))?;

        let req = request.into_inner();

        let organisation_id = req
            .organisation_id
            .parse::<Uuid>()
            .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;

        let page_size = if req.page_size > 0 {
            req.page_size as i64
        } else {
            50
        };
        let offset = req.page_token.parse::<i64>().unwrap_or(0);

        let result = self
            .state
            .organisation_service()
            .list_members(organisation_id, page_size, offset)
            .await
            .map_err(error::to_status)?;

        let next_offset = offset + page_size;
        let next_page_token = if next_offset < result.total_count {
            next_offset.to_string()
        } else {
            String::new()
        };

        Ok(tonic::Response::new(ListMembersResponse {
            members: result.members.into_iter().map(member_to_grpc).collect(),
            next_page_token,
            total_count: result.total_count as i32,
        }))
    }
}

fn member_to_grpc(member: crate::services::organisations::MemberInfo) -> OrganisationMember {
    OrganisationMember {
        user_id: member.user_id.to_string(),
        username: member.username,
        role: member.role,
        joined_at: Some(prost_types::Timestamp {
            seconds: member.joined_at.timestamp(),
            nanos: member.joined_at.timestamp_subsec_nanos() as i32,
        }),
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
