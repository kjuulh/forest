use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use forage_core::platform::validate_slug;
use futures_util::StreamExt;
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::Session;
use crate::forest_client::GrpcForestClient;
use crate::state::AppState;

use super::error_page;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/orgs/{org}/projects/{project}/events",
            get(project_events_sse),
        )
        .route(
            "/api/orgs/{org}/projects/{project}/releases/{slug}/logs",
            get(release_logs_sse),
        )
}

async fn project_events_sse(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    // Validate access
    let orgs = &session.user.orgs;
    if !orgs.iter().any(|o| o.name == org) {
        return Err(error_page(
            &state,
            axum::http::StatusCode::FORBIDDEN,
            "Access denied",
            "You are not a member of this organisation.",
        ));
    }
    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let grpc_client = state.grpc_client.as_ref().ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Service unavailable",
            "Event streaming is not available.",
        )
    })?;

    let access_token = session.access_token.clone();
    let mut event_client = grpc_client.event_client();

    let mut req = tonic::Request::new(forage_grpc::SubscribeEventsRequest {
        organisation: org.clone(),
        project: project.clone(),
        resource_types: vec![],
        actions: vec![],
        since_sequence: 0,
    });
    let bearer: tonic::metadata::MetadataValue<_> = format!("Bearer {access_token}")
        .parse()
        .map_err(|_| {
            error_page(
                &state,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "Failed to create auth header.",
            )
        })?;
    req.metadata_mut().insert("authorization", bearer);

    let grpc_stream = event_client.subscribe(req).await.map_err(|e| {
        tracing::error!("event subscribe failed: {e}");
        error_page(
            &state,
            axum::http::StatusCode::BAD_GATEWAY,
            "Connection failed",
            "Could not connect to event stream.",
        )
    })?;

    let mut grpc_stream = grpc_stream.into_inner();

    // Bridge gRPC stream -> SSE via a channel
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    tokio::spawn(async move {
        while let Some(result) = grpc_stream.next().await {
            match result {
                Ok(event) => {
                    let data = serde_json::json!({
                        "sequence": event.sequence,
                        "event_id": event.event_id,
                        "timestamp": event.timestamp,
                        "organisation": event.organisation,
                        "project": event.project,
                        "resource_type": event.resource_type,
                        "action": event.action,
                        "resource_id": event.resource_id,
                        "metadata": event.metadata,
                    });
                    let sse_event = Event::default()
                        .event(&event.resource_type)
                        .data(data.to_string())
                        .id(event.sequence.to_string());
                    if tx.send(Ok(sse_event)).await.is_err() {
                        break; // Client disconnected
                    }
                }
                Err(e) => {
                    tracing::warn!("event stream error: {e}");
                    break;
                }
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    let sse = Sse::new(stream).keep_alive(KeepAlive::default());

    Ok(sse.into_response())
}

// ─── Release logs SSE ────────────────────────────────────────────────

async fn release_logs_sse(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, slug)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    if !orgs.iter().any(|o| o.name == org) {
        return Err(error_page(
            &state,
            axum::http::StatusCode::FORBIDDEN,
            "Access denied",
            "You are not a member of this organisation.",
        ));
    }
    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let grpc_client = state.grpc_client.as_ref().ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Service unavailable",
            "Log streaming is not available.",
        )
    })?;

    let access_token = session.access_token.clone();

    // Fetch the artifact to get its artifact_id.
    let artifact = state
        .platform_client
        .get_artifact_by_slug(&access_token, &slug)
        .await
        .map_err(|e| {
            tracing::error!("release_logs_sse get_artifact_by_slug: {e}");
            error_page(
                &state,
                axum::http::StatusCode::NOT_FOUND,
                "Not found",
                "Release not found.",
            )
        })?;

    // Fetch release intent states to find intent IDs for this artifact.
    let release_intents = state
        .platform_client
        .get_release_intent_states(&access_token, &org, Some(&project), true)
        .await
        .unwrap_or_default();

    let intent_ids: Vec<String> = release_intents
        .into_iter()
        .filter(|ri| ri.artifact_id == artifact.artifact_id)
        .map(|ri| ri.release_intent_id)
        .collect();

    if intent_ids.is_empty() {
        // No release intents — return an SSE stream that sends a "done" event and closes.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(1);
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(Event::default()
                    .event("done")
                    .data(r#"{"message":"no logs"}"#)))
                .await;
        });
        let stream = ReceiverStream::new(rx);
        return Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response());
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(128);

    // Spawn a WaitRelease stream for each release intent.
    for intent_id in intent_ids {
        let grpc = grpc_client.clone();
        let token = access_token.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = stream_release_logs(&grpc, &token, &intent_id, &tx).await {
                tracing::warn!("release log stream for {intent_id}: {e}");
            }
        });
    }

    // Drop our copy of tx so the stream ends when all spawned tasks finish.
    drop(tx);

    let stream = ReceiverStream::new(rx);
    let sse = Sse::new(stream).keep_alive(KeepAlive::default());
    Ok(sse.into_response())
}

async fn stream_release_logs(
    grpc: &GrpcForestClient,
    access_token: &str,
    release_intent_id: &str,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut client = grpc.release_client();
    let mut req = tonic::Request::new(forage_grpc::WaitReleaseRequest {
        release_intent_id: release_intent_id.to_string(),
    });
    let bearer: tonic::metadata::MetadataValue<_> =
        format!("Bearer {access_token}").parse()?;
    req.metadata_mut().insert("authorization", bearer);

    let resp = client.wait_release(req).await?;
    let mut stream = resp.into_inner();

    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => {
                let sse_event = match event.event {
                    Some(forage_grpc::wait_release_event::Event::LogLine(log)) => {
                        let channel = match log.channel {
                            1 => "stdout",
                            2 => "stderr",
                            _ => "stdout",
                        };
                        let data = serde_json::json!({
                            "destination": log.destination,
                            "line": log.line,
                            "timestamp": log.timestamp,
                            "channel": channel,
                        });
                        Some(Event::default().event("log").data(data.to_string()))
                    }
                    Some(forage_grpc::wait_release_event::Event::StatusUpdate(su)) => {
                        let data = serde_json::json!({
                            "destination": su.destination,
                            "status": su.status,
                        });
                        Some(Event::default().event("status").data(data.to_string()))
                    }
                    Some(forage_grpc::wait_release_event::Event::StageUpdate(su)) => {
                        let data = serde_json::json!({
                            "stage_id": su.stage_id,
                            "stage_type": su.stage_type,
                            "status": su.status,
                        });
                        Some(Event::default().event("stage").data(data.to_string()))
                    }
                    None => None,
                };
                if let Some(sse_event) = sse_event {
                    if tx.send(Ok(sse_event)).await.is_err() {
                        return Ok(()); // Client disconnected
                    }
                }
            }
            Err(e) => {
                tracing::warn!("wait_release stream error: {e}");
                break;
            }
        }
    }

    // Signal that this intent's stream is done.
    let _ = tx
        .send(Ok(Event::default()
            .event("done")
            .data(format!(
                r#"{{"release_intent_id":"{}"}}"#,
                release_intent_id
            ))))
        .await;

    Ok(())
}
