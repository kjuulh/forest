use forest_grpc_interface::{notification_service_server::NotificationService, *};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Response;
use uuid::Uuid;

use crate::{
    actor::Actor,
    grpc::artifacts::GrpcErrorExt,
    services::notification_registry::{NotificationRecord, NotificationRegistryState},
    state::State,
    tokens::AppClaims,
};

pub struct NotificationsServer {
    pub state: State,
}

/// Extract the actor's identity from request extensions.
/// Prefers `AppClaims` (JWT user tokens) and falls back to `Actor`
/// (service accounts and app tokens).
fn extract_actor_id(
    extensions: &http::Extensions,
) -> Result<Uuid, tonic::Status> {
    if let Some(claims) = extensions.get::<AppClaims>() {
        return claims
            .user_id
            .parse()
            .map_err(|_| tonic::Status::internal("invalid user_id in token"));
    }
    if let Some(actor) = extensions.get::<Actor>() {
        return Ok(actor.actor_id());
    }
    Err(tonic::Status::unauthenticated("missing auth context"))
}

#[async_trait::async_trait]
impl NotificationService for NotificationsServer {
    async fn get_notification_preferences(
        &self,
        request: tonic::Request<GetNotificationPreferencesRequest>,
    ) -> std::result::Result<tonic::Response<GetNotificationPreferencesResponse>, tonic::Status>
    {
        let user_id = extract_actor_id(request.extensions())?;

        let prefs = self
            .state
            .notification_registry()
            .get_preferences(&user_id)
            .await
            .to_internal_error()?;

        Ok(Response::new(GetNotificationPreferencesResponse {
            preferences: prefs
                .into_iter()
                .map(|p| NotificationPreference {
                    notification_type: notification_type_from_str(&p.notification_type).into(),
                    channel: notification_channel_from_str(&p.channel).into(),
                    enabled: p.enabled,
                })
                .collect(),
        }))
    }

    async fn set_notification_preference(
        &self,
        request: tonic::Request<SetNotificationPreferenceRequest>,
    ) -> std::result::Result<tonic::Response<SetNotificationPreferenceResponse>, tonic::Status>
    {
        let user_id = extract_actor_id(request.extensions())?;

        let req = request.into_inner();
        let ntype = notification_type_to_str(req.notification_type());
        let channel = notification_channel_to_str(req.channel());

        let pref = self
            .state
            .notification_registry()
            .set_preference(&user_id, ntype, channel, req.enabled)
            .await
            .to_internal_error()?;

        Ok(Response::new(SetNotificationPreferenceResponse {
            preference: Some(NotificationPreference {
                notification_type: notification_type_from_str(&pref.notification_type).into(),
                channel: notification_channel_from_str(&pref.channel).into(),
                enabled: pref.enabled,
            }),
        }))
    }

    type ListenNotificationsStream = ReceiverStream<Result<Notification, tonic::Status>>;

    async fn listen_notifications(
        &self,
        request: tonic::Request<ListenNotificationsRequest>,
    ) -> std::result::Result<tonic::Response<Self::ListenNotificationsStream>, tonic::Status> {
        tracing::debug!("listen_notifications stream");

        let user_id = extract_actor_id(request.extensions())?;

        let req = request.into_inner();
        let organisation = req.organisation;
        let project = req.project;

        let (tx, rx) = mpsc::channel(32);
        let registry = self.state.notification_registry();

        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_secs(2);
            // Start from the current max sequence so we only deliver new notifications
            let mut last_sequence: i64 = registry
                .get_max_sequence()
                .await
                .unwrap_or(0);

            loop {
                match registry
                    .poll_notifications(
                        &user_id,
                        last_sequence,
                        organisation.as_deref(),
                        project.as_deref(),
                        50,
                    )
                    .await
                {
                    Ok(notifications) => {
                        for notif in notifications {
                            if notif.sequence > last_sequence {
                                last_sequence = notif.sequence;
                            }
                            let grpc_notif = notification_record_to_grpc(notif);
                            if tx.send(Ok(grpc_notif)).await.is_err() {
                                return; // Client disconnected
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("error polling notifications: {e:#}");
                        let _ = tx
                            .send(Err(tonic::Status::internal(format!(
                                "error polling notifications: {e}"
                            ))))
                            .await;
                        return;
                    }
                }

                tokio::time::sleep(poll_interval).await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn list_notifications(
        &self,
        request: tonic::Request<ListNotificationsRequest>,
    ) -> std::result::Result<tonic::Response<ListNotificationsResponse>, tonic::Status> {
        let user_id = extract_actor_id(request.extensions())?;

        let req = request.into_inner();
        let limit = if req.page_size > 0 {
            req.page_size as i64
        } else {
            50
        };

        // Return the most recent notifications (newest first)
        let notifications = self
            .state
            .notification_registry()
            .list_recent_notifications(
                &user_id,
                req.organisation.as_deref(),
                req.project.as_deref(),
                limit,
            )
            .await
            .to_internal_error()?;

        let next_page_token = notifications
            .last()
            .map(|n| n.sequence.to_string())
            .unwrap_or_default();

        Ok(Response::new(ListNotificationsResponse {
            notifications: notifications
                .into_iter()
                .map(notification_record_to_grpc)
                .collect(),
            next_page_token,
        }))
    }
}

fn notification_type_to_str(t: NotificationType) -> &'static str {
    match t {
        NotificationType::ReleaseAnnotated => "RELEASE_ANNOTATED",
        NotificationType::ReleaseStarted => "RELEASE_STARTED",
        NotificationType::ReleaseSucceeded => "RELEASE_SUCCEEDED",
        NotificationType::ReleaseFailed => "RELEASE_FAILED",
        NotificationType::Unspecified => "UNSPECIFIED",
    }
}

fn notification_type_from_str(s: &str) -> NotificationType {
    match s {
        "RELEASE_ANNOTATED" => NotificationType::ReleaseAnnotated,
        "RELEASE_STARTED" => NotificationType::ReleaseStarted,
        "RELEASE_SUCCEEDED" => NotificationType::ReleaseSucceeded,
        "RELEASE_FAILED" => NotificationType::ReleaseFailed,
        _ => NotificationType::Unspecified,
    }
}

fn notification_channel_to_str(c: NotificationChannel) -> &'static str {
    match c {
        NotificationChannel::Cli => "CLI",
        NotificationChannel::Slack => "SLACK",
        NotificationChannel::Unspecified => "CLI",
    }
}

fn notification_channel_from_str(s: &str) -> NotificationChannel {
    match s {
        "CLI" => NotificationChannel::Cli,
        "SLACK" => NotificationChannel::Slack,
        _ => NotificationChannel::Unspecified,
    }
}

fn release_context_to_grpc(
    ctx: crate::services::notification_registry::ReleaseContext,
) -> ReleaseContext {
    ReleaseContext {
        slug: ctx.slug.unwrap_or_default(),
        organisation: String::new(), // populated from top-level fields
        project: String::new(),
        artifact_id: ctx.artifact_id.unwrap_or_default(),
        release_intent_id: ctx.release_intent_id.unwrap_or_default(),
        destination: ctx.destination.unwrap_or_default(),
        environment: ctx.environment.unwrap_or_default(),
        source_username: ctx.source_username.unwrap_or_default(),
        source_email: ctx.source_email.unwrap_or_default(),
        source_user_id: ctx.source_user_id.unwrap_or_default(),
        commit_sha: ctx.commit_sha.unwrap_or_default(),
        commit_branch: ctx.commit_branch.unwrap_or_default(),
        context_title: ctx.context_title.unwrap_or_default(),
        context_description: ctx.context_description.unwrap_or_default(),
        context_web: ctx.context_web.unwrap_or_default(),
        error_message: ctx.error_message.unwrap_or_default(),
        destination_count: ctx.destination_count,
    }
}

fn notification_record_to_grpc(r: NotificationRecord) -> Notification {
    Notification {
        id: r.id.to_string(),
        notification_type: notification_type_from_str(&r.notification_type).into(),
        title: r.title,
        body: r.body,
        organisation: r.organisation.clone(),
        project: r.project.clone(),
        release_context: Some(release_context_to_grpc(r.release_context)),
        created_at: r.created_at.to_rfc3339(),
    }
}
