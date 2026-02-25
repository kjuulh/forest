use forest_grpc_interface::{Notification, NotificationType};

pub fn format_notification(notif: &Notification) -> String {
    let ntype = format_type(notif.notification_type());
    let ts = format_timestamp(&notif.created_at);

    let mut out = format!(
        "[{ntype}] {title}\n  {org}/{project} | {ts}",
        title = notif.title,
        org = notif.organisation,
        project = notif.project,
    );

    if let Some(ctx) = &notif.release_context {
        // User who triggered the action
        if !ctx.source_username.is_empty() {
            let user = if !ctx.source_email.is_empty() {
                format!("{} <{}>", ctx.source_username, ctx.source_email)
            } else {
                ctx.source_username.clone()
            };
            out.push_str(&format!("\n  by {user}"));
        }

        // Context title / description (PR title, release label, etc.)
        if !ctx.context_title.is_empty() {
            out.push_str(&format!("\n  title: {}", ctx.context_title));
            if !ctx.context_description.is_empty() {
                out.push_str(&format!("\n         {}", ctx.context_description));
            }
        }

        // Git info
        if !ctx.commit_sha.is_empty() || !ctx.commit_branch.is_empty() {
            let sha = if ctx.commit_sha.len() > 7 {
                &ctx.commit_sha[..7]
            } else if !ctx.commit_sha.is_empty() {
                &ctx.commit_sha
            } else {
                ""
            };
            let branch = &ctx.commit_branch;
            match (sha.is_empty(), branch.is_empty()) {
                (false, false) => out.push_str(&format!("\n  commit: {sha} ({branch})")),
                (false, true) => out.push_str(&format!("\n  commit: {sha}")),
                (true, false) => out.push_str(&format!("\n  branch: {branch}")),
                _ => {}
            }
        }

        // Slug
        if !ctx.slug.is_empty() {
            out.push_str(&format!("\n  slug: {}", ctx.slug));
        }

        // Destination info
        if !ctx.destination.is_empty() {
            let env_suffix = if !ctx.environment.is_empty() {
                format!(" ({})", ctx.environment)
            } else {
                String::new()
            };
            out.push_str(&format!("\n  destination: {}{env_suffix}", ctx.destination));
        } else if ctx.destination_count > 0 {
            out.push_str(&format!(
                "\n  destinations: {}",
                ctx.destination_count
            ));
        }

        // Web link
        if !ctx.context_web.is_empty() {
            out.push_str(&format!("\n  link: {}", ctx.context_web));
        }

        // Error (for failures)
        if !ctx.error_message.is_empty() {
            out.push_str(&format!("\n  error: {}", ctx.error_message));
        }
    }

    out
}

fn format_type(t: NotificationType) -> &'static str {
    match t {
        NotificationType::ReleaseAnnotated => "ANNOTATED",
        NotificationType::ReleaseStarted => "STARTED",
        NotificationType::ReleaseSucceeded => "SUCCEEDED",
        NotificationType::ReleaseFailed => "FAILED",
        NotificationType::Unspecified => "UNKNOWN",
    }
}

fn format_timestamp(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| ts.to_string())
}
