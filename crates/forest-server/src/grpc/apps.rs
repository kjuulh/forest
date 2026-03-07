use forest_grpc_interface::{app_service_server::AppService, *};
use uuid::Uuid;

use crate::{
    actor::Actor,
    grpc::artifacts::GrpcErrorExt,
    services::apps::AppServiceState,
    state::State,
};

pub struct AppsServer {
    pub state: State,
}

fn extract_actor(request: &tonic::Request<impl std::any::Any>) -> Result<Actor, tonic::Status> {
    request
        .extensions()
        .get::<Actor>()
        .cloned()
        .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))
}

fn require_user(actor: &Actor) -> Result<Uuid, tonic::Status> {
    match actor {
        Actor::User { user_id } => Ok(*user_id),
        _ => Err(tonic::Status::permission_denied(
            "only users can manage apps",
        )),
    }
}

#[async_trait::async_trait]
impl AppService for AppsServer {
    async fn create_app(
        &self,
        request: tonic::Request<CreateAppRequest>,
    ) -> Result<tonic::Response<CreateAppResponse>, tonic::Status> {
        let actor = extract_actor(&request)?;
        let user_id = require_user(&actor)?;
        let req = request.into_inner();

        let org_id: Uuid = req
            .organisation_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;

        let permissions =
            serde_json::to_value(&req.permissions).map_err(|e| tonic::Status::internal(e.to_string()))?;

        let app = self
            .state
            .app_service()
            .create_app(
                org_id,
                &req.name,
                if req.description.is_empty() {
                    None
                } else {
                    Some(&req.description)
                },
                &permissions,
                user_id,
            )
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(CreateAppResponse {
            app: Some(app_to_grpc(app)),
        }))
    }

    async fn get_app(
        &self,
        request: tonic::Request<GetAppRequest>,
    ) -> Result<tonic::Response<GetAppResponse>, tonic::Status> {
        let _actor = extract_actor(&request)?;
        let req = request.into_inner();

        let app_id: Uuid = req
            .app_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid app_id"))?;

        let app = self
            .state
            .app_service()
            .get_app(app_id)
            .await
            .to_internal_error()?
            .ok_or_else(|| tonic::Status::not_found("app not found"))?;

        Ok(tonic::Response::new(GetAppResponse {
            app: Some(app_to_grpc(app)),
        }))
    }

    async fn list_apps(
        &self,
        request: tonic::Request<ListAppsRequest>,
    ) -> Result<tonic::Response<ListAppsResponse>, tonic::Status> {
        let _actor = extract_actor(&request)?;
        let req = request.into_inner();

        let org_id: Uuid = req
            .organisation_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid organisation_id"))?;

        let apps = self
            .state
            .app_service()
            .list_apps(org_id)
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(ListAppsResponse {
            apps: apps.into_iter().map(app_to_grpc).collect(),
        }))
    }

    async fn delete_app(
        &self,
        request: tonic::Request<DeleteAppRequest>,
    ) -> Result<tonic::Response<DeleteAppResponse>, tonic::Status> {
        let actor = extract_actor(&request)?;
        let _user_id = require_user(&actor)?;
        let req = request.into_inner();

        let app_id: Uuid = req
            .app_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid app_id"))?;

        self.state
            .app_service()
            .delete_app(app_id)
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(DeleteAppResponse {}))
    }

    async fn suspend_app(
        &self,
        request: tonic::Request<SuspendAppRequest>,
    ) -> Result<tonic::Response<SuspendAppResponse>, tonic::Status> {
        let actor = extract_actor(&request)?;
        let _user_id = require_user(&actor)?;
        let req = request.into_inner();

        let app_id: Uuid = req
            .app_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid app_id"))?;

        self.state
            .app_service()
            .suspend_app(app_id, req.suspended)
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(SuspendAppResponse {}))
    }

    // -- Token management ---------------------------------------------------------

    async fn create_app_token(
        &self,
        request: tonic::Request<CreateAppTokenRequest>,
    ) -> Result<tonic::Response<CreateAppTokenResponse>, tonic::Status> {
        let actor = extract_actor(&request)?;
        let _user_id = require_user(&actor)?;
        let req = request.into_inner();

        let app_id: Uuid = req
            .app_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid app_id"))?;

        let expires_at = if req.expires_in_seconds > 0 {
            Some(chrono::Utc::now() + chrono::Duration::seconds(req.expires_in_seconds))
        } else {
            None
        };

        let created = self
            .state
            .app_service()
            .create_token(app_id, &req.name, expires_at)
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(CreateAppTokenResponse {
            token: Some(AppToken {
                token_id: created.token_id.to_string(),
                name: created.name,
                expires_at: created.expires_at.map(datetime_to_timestamp),
                last_used: None,
                revoked: false,
                created_at: Some(datetime_to_timestamp(created.created_at)),
            }),
            raw_token: created.raw_token,
        }))
    }

    async fn list_app_tokens(
        &self,
        request: tonic::Request<ListAppTokensRequest>,
    ) -> Result<tonic::Response<ListAppTokensResponse>, tonic::Status> {
        let _actor = extract_actor(&request)?;
        let req = request.into_inner();

        let app_id: Uuid = req
            .app_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid app_id"))?;

        let tokens = self
            .state
            .app_service()
            .list_tokens(app_id)
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(ListAppTokensResponse {
            tokens: tokens.into_iter().map(token_to_grpc).collect(),
        }))
    }

    async fn revoke_app_token(
        &self,
        request: tonic::Request<RevokeAppTokenRequest>,
    ) -> Result<tonic::Response<RevokeAppTokenResponse>, tonic::Status> {
        let actor = extract_actor(&request)?;
        let _user_id = require_user(&actor)?;
        let req = request.into_inner();

        let token_id: Uuid = req
            .token_id
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("invalid token_id"))?;

        self.state
            .app_service()
            .revoke_token(token_id)
            .await
            .to_internal_error()?;

        Ok(tonic::Response::new(RevokeAppTokenResponse {}))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn app_to_grpc(app: crate::services::apps::AppInfo) -> App {
    let permissions: Vec<String> = serde_json::from_value(app.permissions).unwrap_or_default();
    App {
        app_id: app.id.to_string(),
        organisation_id: app.organisation_id.to_string(),
        name: app.name,
        description: app.description.unwrap_or_default(),
        permissions,
        suspended: app.suspended,
        created_at: Some(datetime_to_timestamp(app.created_at)),
    }
}

fn token_to_grpc(t: crate::services::apps::AppTokenInfo) -> AppToken {
    AppToken {
        token_id: t.id.to_string(),
        name: t.name,
        expires_at: t.expires_at.map(datetime_to_timestamp),
        last_used: t.last_used.map(datetime_to_timestamp),
        revoked: t.revoked,
        created_at: Some(datetime_to_timestamp(t.created_at)),
    }
}

fn datetime_to_timestamp(dt: chrono::DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}
