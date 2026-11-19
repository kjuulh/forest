use forage_core::platform::{Policy, PolicyConfig, ReleasePipeline};
use forage_ui::chrome::Chrome;
use forage_ui::format::urlencode;
use forage_ui::policies::PolicyKind;

use crate::views::models::project_href;

pub struct PoliciesPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub policies: Vec<Policy>,
    pub environments: Vec<String>,
    pub policy_form_js: String,
}

pub struct PolicyEditPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub policy: Policy,
    pub environments: Vec<String>,
}

pub struct PipelinesPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub pipelines: Vec<PipelineRow>,
    pub pipeline_builder_js: String,
}

pub struct PipelineRow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub stage_count: usize,
    pub stages_json: String,
    pub updated_at: String,
}

impl From<ReleasePipeline> for PipelineRow {
    fn from(p: ReleasePipeline) -> Self {
        Self {
            stage_count: p.stages.len(),
            stages_json: serde_json::to_string(&p.stages).unwrap_or_default(),
            id: p.id,
            name: p.name,
            enabled: p.enabled,
            updated_at: p.updated_at,
        }
    }
}

pub fn policy_kind(config: &PolicyConfig) -> PolicyKind {
    match config {
        PolicyConfig::SoakTime { .. } => PolicyKind::SoakTime,
        PolicyConfig::BranchRestriction { .. } => PolicyKind::BranchRestriction,
        PolicyConfig::Approval { .. } => PolicyKind::Approval,
        PolicyConfig::SupersedePending { .. } => PolicyKind::SupersedePending,
    }
}

pub fn policies_href(org: &str, project: &str) -> String {
    format!("{}/settings/policies", project_href(org, project))
}

pub fn policy_href(org: &str, project: &str, id: &str) -> String {
    format!("{}/{}", policies_href(org, project), urlencode(id))
}

pub fn pipelines_href(org: &str, project: &str) -> String {
    format!("{}/settings/pipelines", project_href(org, project))
}

pub fn pipeline_href(org: &str, project: &str, id: &str) -> String {
    format!("{}/{}", pipelines_href(org, project), urlencode(id))
}

pub fn plural(n: impl Into<i64>) -> &'static str {
    if n.into() == 1 { "" } else { "s" }
}

pub fn duration_human(seconds: i64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        if mins > 0 {
            format!("{hours}h {mins}m")
        } else {
            format!("{hours}h")
        }
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}
