use forest_models::ReleaseStatus;
use uuid::Uuid;

use super::{
    destination_registry::DestinationRegistry,
    notification_registry::{NotificationRegistry, ReleaseContext as NotifReleaseContext},
    release_registry::{ReleaseItem, ReleaseRegistry},
};

/// Update release status AND create a notification.
/// Used by the CompleteRelease gRPC handler (runner path).
pub async fn finalize_release(
    release_registry: &ReleaseRegistry,
    notification_registry: &NotificationRegistry,
    destination_registry: &DestinationRegistry,
    release_id: &Uuid,
    release_intent_id: &Uuid,
    artifact_id: &Uuid,
    project_id: &Uuid,
    destination_id: &Uuid,
    status: ReleaseStatus,
    error_message: Option<&str>,
) -> anyhow::Result<()> {
    release_registry
        .set_release_status(release_id, status)
        .await?;

    create_release_notification(
        release_registry,
        notification_registry,
        destination_registry,
        release_id,
        release_intent_id,
        artifact_id,
        project_id,
        destination_id,
        status,
        error_message,
    )
    .await
}

/// Create a release notification without updating status.
/// Used by the Scheduler (in-process path) which manages its own status via transactions.
pub async fn send_notification(
    release_registry: &ReleaseRegistry,
    notification_registry: &NotificationRegistry,
    destination_registry: &DestinationRegistry,
    release_item: &ReleaseItem,
    status: ReleaseStatus,
    error_message: Option<&str>,
) -> anyhow::Result<()> {
    create_release_notification(
        release_registry,
        notification_registry,
        destination_registry,
        &release_item.id,
        &release_item.release_intent_id,
        &release_item.artifact,
        &release_item.project_id,
        &release_item.destination_id,
        status,
        error_message,
    )
    .await
}

async fn create_release_notification(
    release_registry: &ReleaseRegistry,
    notification_registry: &NotificationRegistry,
    destination_registry: &DestinationRegistry,
    release_id: &Uuid,
    release_intent_id: &Uuid,
    artifact_id: &Uuid,
    project_id: &Uuid,
    destination_id: &Uuid,
    status: ReleaseStatus,
    error_message: Option<&str>,
) -> anyhow::Result<()> {
    let project_context = release_registry
        .get_project_context(project_id)
        .await
        .ok();

    let dest = destination_registry.get(destination_id).await.ok().flatten();
    let dest_name = dest.as_ref().map(|d| d.name.clone());
    let dest_env = dest.as_ref().map(|d| d.environment.clone());

    let ann_ctx = release_registry
        .get_annotation_context(artifact_id)
        .await
        .ok();

    let Some((ref org, ref project)) = project_context else {
        return Ok(());
    };

    let (notification_type, title, body) = if status.is_success() {
        (
            "RELEASE_SUCCEEDED",
            format!("Release succeeded: {}/{}", org, project),
            format!(
                "Release {} completed successfully{}",
                release_id,
                dest_name
                    .as_ref()
                    .map(|d| format!(" to {}", d))
                    .unwrap_or_default()
            ),
        )
    } else {
        (
            "RELEASE_FAILED",
            format!("Release failed: {}/{}", org, project),
            format!(
                "Release {} failed: {}{}",
                release_id,
                error_message.unwrap_or("unknown error"),
                dest_name
                    .as_ref()
                    .map(|d| format!(" (dest: {})", d))
                    .unwrap_or_default()
            ),
        )
    };

    let release_context = NotifReleaseContext {
        slug: ann_ctx.as_ref().map(|a| a.slug.clone()),
        artifact_id: Some(artifact_id.to_string()),
        release_intent_id: Some(release_intent_id.to_string()),
        destination: dest_name,
        environment: dest_env,
        source_username: ann_ctx
            .as_ref()
            .and_then(|a| a.source.username.clone()),
        source_email: ann_ctx.as_ref().and_then(|a| a.source.email.clone()),
        source_type: ann_ctx
            .as_ref()
            .and_then(|a| a.source.source_type.clone()),
        run_url: ann_ctx.as_ref().and_then(|a| a.source.run_url.clone()),
        commit_sha: ann_ctx
            .as_ref()
            .map(|a| a.reference.commit_sha.clone()),
        commit_branch: ann_ctx
            .as_ref()
            .and_then(|a| a.reference.commit_branch.clone()),
        commit_message: ann_ctx
            .as_ref()
            .and_then(|a| a.reference.commit_message.clone()),
        version: ann_ctx
            .as_ref()
            .and_then(|a| a.reference.version.clone()),
        repo_url: ann_ctx
            .as_ref()
            .and_then(|a| a.reference.repo_url.clone()),
        context_title: ann_ctx
            .as_ref()
            .map(|a| a.context.title.clone()),
        context_description: ann_ctx
            .as_ref()
            .and_then(|a| a.context.description.clone()),
        context_web: ann_ctx
            .as_ref()
            .and_then(|a| a.context.web.clone()),
        context_pr: ann_ctx.as_ref().and_then(|a| a.context.pr.clone()),
        error_message: error_message.map(|e| e.to_string()),
        ..Default::default()
    };

    if let Err(e) = notification_registry
        .create_notification(
            notification_type,
            &title,
            &body,
            org,
            project,
            &release_context,
        )
        .await
    {
        tracing::warn!("failed to create {} notification: {e:#}", notification_type);
    }

    Ok(())
}
