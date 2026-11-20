use std::collections::HashMap;

use forage_core::platform::{ApprovalState, Artifact};
use forage_ui::chrome::Chrome;
use forage_ui::format::urlencode;
use forage_ui::release::{DestinationRun, RunStatus, Stage};

use crate::views::models::project_href;

const TITLE_LIMIT: usize = 120;
const DESCRIPTION_LIMIT: usize = 800;

pub struct ReleasePage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub release: ReleaseDetails,
    pub summary: RunStatus,
    pub environments: Vec<String>,
    pub has_pipeline: bool,
    pub stages: Vec<Stage>,
    pub release_intent_id: String,
    pub policies: Vec<PolicyCheck>,
    pub destinations: Vec<DestinationRun>,
    pub configured_destinations: Vec<ConfiguredDestination>,
    pub has_release_intents: bool,
    pub spec: Option<String>,
    pub health: Option<forage_core::platform::ReleaseHealth>,
}

// Class names stay whole literals: Tailwind scans this directory for them.
pub fn health_classes(status: &str) -> (&'static str, &'static str, &'static str) {
    match status {
        "HEALTHY" => (
            "bg-green-100 text-green-800",
            "bg-green-500",
            "text-green-600",
        ),
        "PROGRESSING" => (
            "bg-yellow-100 text-yellow-800",
            "bg-yellow-500 animate-pulse",
            "text-yellow-600",
        ),
        "DEGRADED" => (
            "bg-orange-100 text-orange-800",
            "bg-orange-500",
            "text-orange-600",
        ),
        "UNHEALTHY" | "MISSING" => ("bg-red-100 text-red-800", "bg-red-500", "text-red-600"),
        _ => ("bg-gray-100 text-gray-700", "bg-gray-400", "text-gray-500"),
    }
}

pub fn health_label(status: &str) -> String {
    let mut chars = status.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

pub fn stages_done(stages: &[Stage]) -> usize {
    stages
        .iter()
        .filter(|s| s.status == RunStatus::Succeeded)
        .count()
}

pub fn policies_passed(policies: &[PolicyCheck]) -> usize {
    policies.iter().filter(|p| p.passed).count()
}

pub fn release_href(org: &str, project: &str, slug: &str) -> String {
    format!(
        "{}/releases/{}",
        project_href(org, project),
        urlencode(slug)
    )
}

pub fn plan_stage_href(org: &str, project: &str, stage_id: &str, action: &str) -> String {
    format!(
        "/api/orgs/{}/projects/{}/plan-stages/{}/{action}",
        urlencode(org),
        urlencode(project),
        urlencode(stage_id)
    )
}

pub struct ReleaseDetails {
    pub artifact_id: String,
    pub slug: String,
    pub heading: String,
    pub full_title: String,
    pub description: Option<Excerpt>,
    pub version: Option<String>,
    pub branch: Option<String>,
    pub short_sha: Option<String>,
    pub source_type: Option<String>,
    pub source_user: Option<String>,
    pub created_at: String,
    pub commit_message: Option<String>,
    pub repo_url: Option<String>,
    pub source_email: Option<String>,
    pub run_url: Option<String>,
    pub web: Option<String>,
    pub pr: Option<String>,
}

impl ReleaseDetails {
    pub fn has_details(&self) -> bool {
        self.commit_message.is_some()
            || self.repo_url.is_some()
            || self.source_email.is_some()
            || self.run_url.is_some()
    }
}

pub struct Excerpt {
    pub full: String,
    pub shown: String,
}

pub struct PolicyCheck {
    pub name: String,
    pub policy_type: String,
    pub passed: bool,
    pub reason: String,
    pub target_environment: String,
    pub approval: Option<ApprovalState>,
}

impl PolicyCheck {
    pub fn approval_state(&self) -> Option<&ApprovalState> {
        self.approval
            .as_ref()
            .filter(|_| self.policy_type == "approval")
    }
}

pub struct ConfiguredDestination {
    pub name: String,
    pub environment: String,
}

fn present(value: Option<&String>) -> Option<String> {
    value.filter(|v| !v.is_empty()).cloned()
}

fn truncate(text: &str, limit: usize) -> (String, bool) {
    let mut chars = text.chars();
    let shown: String = chars.by_ref().take(limit).collect();
    (shown, chars.next().is_some())
}

pub fn details(artifact: &Artifact, description_meta: &HashMap<String, String>) -> ReleaseDetails {
    let git = artifact.git_ref.as_ref();
    let source = artifact.source.as_ref();
    let commit_message = present(git.and_then(|r| r.commit_message.as_ref()));

    let full_title = commit_message
        .clone()
        .unwrap_or_else(|| artifact.context.title.clone());
    let first_line = full_title.split('\n').next().unwrap_or_default();
    let (mut heading, cut) = truncate(first_line, TITLE_LIMIT);
    if cut || full_title.contains('\n') {
        heading.push('…');
    }

    let description = artifact
        .context
        .description
        .as_deref()
        .filter(|d| !d.is_empty() && !d.starts_with("Branch:"))
        .map(|d| {
            let (mut shown, cut) = truncate(d, DESCRIPTION_LIMIT);
            if cut {
                shown.push('…');
            }
            Excerpt {
                full: d.to_string(),
                shown,
            }
        });

    ReleaseDetails {
        artifact_id: artifact.artifact_id.clone(),
        slug: artifact.slug.clone(),
        heading,
        full_title,
        description,
        version: present(git.and_then(|r| r.version.as_ref())),
        branch: present(
            git.and_then(|r| r.branch.as_ref())
                .or(description_meta.get("branch")),
        ),
        short_sha: git
            .map(|r| r.commit_sha.chars().take(8).collect::<String>())
            .filter(|s| !s.is_empty()),
        source_type: present(
            source
                .and_then(|s| s.source_type.as_ref())
                .or(description_meta.get("source")),
        ),
        source_user: present(
            source
                .and_then(|s| s.user.as_ref())
                .or(description_meta.get("author")),
        ),
        created_at: artifact.created_at.clone(),
        commit_message,
        repo_url: present(git.and_then(|r| r.repo_url.as_ref())),
        source_email: present(source.and_then(|s| s.email.as_ref())),
        run_url: present(source.and_then(|s| s.run_url.as_ref())),
        web: present(artifact.context.web.as_ref()),
        pr: present(artifact.context.pr.as_ref()),
    }
}

pub fn user_href(username: &str) -> String {
    format!("/users/{}", urlencode(username))
}

pub fn deploy_href(org: &str, project: &str) -> String {
    format!("{}/deploy", project_href(org, project))
}

pub fn logs_href(org: &str, project: &str, slug: &str) -> String {
    format!(
        "/api/orgs/{}/projects/{}/releases/{}/logs",
        urlencode(org),
        urlencode(project),
        urlencode(slug)
    )
}
