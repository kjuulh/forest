use crate::Tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    Pending,
    Queued,
    Running,
    AwaitingApproval,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Other,
}

impl RunStatus {
    pub fn parse(status: &str) -> Self {
        match status {
            "SUCCEEDED" => RunStatus::Succeeded,
            "RUNNING" | "ASSIGNED" => RunStatus::Running,
            "QUEUED" => RunStatus::Queued,
            "FAILED" => RunStatus::Failed,
            "TIMED_OUT" => RunStatus::TimedOut,
            "CANCELLED" => RunStatus::Cancelled,
            "AWAITING_APPROVAL" | "AWAITINGAPPROVAL" => RunStatus::AwaitingApproval,
            "PENDING" | "" => RunStatus::Pending,
            _ => RunStatus::Other,
        }
    }

    pub fn dot_classes(self) -> &'static str {
        match self {
            RunStatus::Running => "bg-yellow-400 animate-pulse",
            RunStatus::Queued => "bg-blue-400",
            RunStatus::Failed => "bg-red-500",
            RunStatus::Succeeded => "bg-green-500",
            RunStatus::TimedOut => "bg-orange-400",
            RunStatus::Cancelled => "bg-gray-400",
            RunStatus::Pending | RunStatus::AwaitingApproval | RunStatus::Other => "bg-gray-300",
        }
    }

    pub fn destination_label(self) -> Option<(&'static str, &'static str)> {
        match self {
            RunStatus::Succeeded => Some(("Deployed", "text-green-600")),
            RunStatus::Running => Some(("Deploying", "text-yellow-600")),
            RunStatus::Queued => Some(("Queued", "text-blue-600")),
            RunStatus::Failed => Some(("Failed", "text-red-600")),
            RunStatus::TimedOut => Some(("Timed out", "text-orange-600")),
            RunStatus::Cancelled => Some(("Cancelled", "text-gray-500")),
            RunStatus::Pending | RunStatus::AwaitingApproval | RunStatus::Other => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StageKind {
    #[default]
    Other,
    Deploy,
    Wait,
    Plan,
}

impl StageKind {
    pub fn parse(kind: &str) -> Self {
        match kind {
            "deploy" => StageKind::Deploy,
            "wait" => StageKind::Wait,
            "plan" => StageKind::Plan,
            _ => StageKind::Other,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stage {
    pub id: String,
    pub anchor: String,
    pub kind: StageKind,
    pub environment: String,
    pub status: RunStatus,
    pub duration_seconds: Option<i64>,
    pub started_at: Option<String>,
    pub error_message: Option<String>,
    pub plan_output: Option<String>,
}

impl Stage {
    pub fn awaiting_approval(&self) -> bool {
        self.status == RunStatus::AwaitingApproval
    }

    pub fn shows_environment(&self) -> bool {
        matches!(self.kind, StageKind::Deploy | StageKind::Plan)
    }

    pub fn label(&self) -> Option<String> {
        let s = self.status;
        let text = match self.kind {
            StageKind::Deploy => match s {
                RunStatus::Succeeded => "Deployed to",
                RunStatus::Running => "Deploying to",
                RunStatus::Queued => "Queued for",
                RunStatus::Failed => "Failed on",
                RunStatus::TimedOut => "Timed out on",
                RunStatus::Cancelled => "Cancelled",
                _ => "Deploy to",
            },
            StageKind::Wait => {
                let verb = match s {
                    RunStatus::Succeeded => "Waited",
                    RunStatus::Running => "Waiting",
                    RunStatus::Failed => "Wait failed",
                    RunStatus::Cancelled => "Wait cancelled",
                    _ => "Wait",
                };
                return Some(match self.duration_seconds.filter(|d| *d != 0) {
                    Some(d) => format!("{verb} {d}s"),
                    None => verb.to_string(),
                });
            }
            StageKind::Plan => match s {
                RunStatus::Succeeded => "Plan approved",
                RunStatus::AwaitingApproval => "Awaiting plan approval",
                RunStatus::Running => "Planning",
                RunStatus::Failed => "Plan failed",
                RunStatus::Cancelled => "Plan cancelled",
                _ => "Plan",
            },
            StageKind::Other => return None,
        };
        Some(text.to_string())
    }

    pub fn label_class(&self) -> &'static str {
        match (self.kind, self.status) {
            (StageKind::Plan, RunStatus::AwaitingApproval) => "text-purple-700",
            (_, RunStatus::Succeeded) => "text-gray-700",
            (_, RunStatus::Running) => "text-yellow-700",
            (StageKind::Deploy | StageKind::Plan, RunStatus::Failed) => "text-red-700",
            _ => "text-gray-400",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DestinationRun {
    pub name: String,
    pub anchor: String,
    pub environment: String,
    pub status: RunStatus,
    pub error_message: Option<String>,
    pub queue_position: Option<i32>,
    pub at: Option<String>,
}

impl DestinationRun {
    pub fn status_text(&self) -> Option<(String, &'static str)> {
        let (label, class) = self.status.destination_label()?;
        let text = match self.queue_position.filter(|p| *p != 0) {
            Some(p) if self.status == RunStatus::Queued => format!("{label} #{p}"),
            _ => label.to_string(),
        };
        Some((text, class))
    }
}

pub fn outline_chip_classes(tone: Tone) -> &'static str {
    match tone {
        Tone::Blue => "font-medium bg-blue-50 text-blue-700 border-blue-200",
        Tone::Purple => "font-medium bg-purple-50 text-purple-700 border-purple-200",
        Tone::Amber => {
            "font-medium bg-amber-50 text-amber-700 border-amber-200 hover:bg-amber-100 transition-colors"
        }
        _ => "font-mono bg-gray-100 text-gray-600 border-gray-200",
    }
}

pub fn source_icon(source_type: &str) -> &'static str {
    if source_type.contains("github") {
        "github"
    } else if source_type.contains("gitlab") {
        "gitlab"
    } else {
        "cog"
    }
}

pub fn source_label(source_type: &str) -> String {
    source_type.replace(['_', '-'], " ")
}

pub fn policy_type_badge(policy_type: &str) -> Option<(&'static str, &'static str)> {
    match policy_type {
        "approval" => Some(("Approval", "bg-emerald-100 text-emerald-700")),
        "soak_time" => Some(("Soak Time", "bg-indigo-100 text-indigo-700")),
        "branch_restriction" => Some(("Branch", "bg-orange-100 text-orange-700")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(kind: StageKind, status: RunStatus) -> Stage {
        Stage {
            kind,
            status,
            ..Default::default()
        }
    }

    #[test]
    fn statuses_parse_both_approval_spellings_and_assigned_as_running() {
        assert_eq!(
            RunStatus::parse("AWAITINGAPPROVAL"),
            RunStatus::AwaitingApproval
        );
        assert_eq!(
            RunStatus::parse("AWAITING_APPROVAL"),
            RunStatus::AwaitingApproval
        );
        assert_eq!(RunStatus::parse("ASSIGNED"), RunStatus::Running);
        assert_eq!(RunStatus::parse("whatever"), RunStatus::Other);
    }

    #[test]
    fn stage_labels_follow_kind_and_status() {
        assert_eq!(
            stage(StageKind::Deploy, RunStatus::Queued)
                .label()
                .as_deref(),
            Some("Queued for")
        );
        assert_eq!(
            stage(StageKind::Plan, RunStatus::AwaitingApproval)
                .label()
                .as_deref(),
            Some("Awaiting plan approval")
        );
        let wait = Stage {
            duration_seconds: Some(30),
            ..stage(StageKind::Wait, RunStatus::Succeeded)
        };
        assert_eq!(wait.label().as_deref(), Some("Waited 30s"));
        let zero = Stage {
            duration_seconds: Some(0),
            ..stage(StageKind::Wait, RunStatus::Pending)
        };
        assert_eq!(zero.label().as_deref(), Some("Wait"));
        assert_eq!(stage(StageKind::Other, RunStatus::Running).label(), None);
    }

    #[test]
    fn a_wait_that_failed_is_not_red() {
        assert_eq!(
            stage(StageKind::Wait, RunStatus::Failed).label_class(),
            "text-gray-400"
        );
        assert_eq!(
            stage(StageKind::Deploy, RunStatus::Failed).label_class(),
            "text-red-700"
        );
    }

    #[test]
    fn a_queued_destination_shows_its_position() {
        let dest = DestinationRun {
            status: RunStatus::Queued,
            queue_position: Some(3),
            ..Default::default()
        };
        assert_eq!(
            dest.status_text(),
            Some(("Queued #3".into(), "text-blue-600"))
        );
        let pending = DestinationRun::default();
        assert_eq!(pending.status_text(), None);
    }
}

#[cfg(test)]
mod status_tests {
    use super::RunStatus;

    #[test]
    fn only_a_pending_stage_reads_as_pending() {
        assert_eq!(RunStatus::parse("PENDING"), RunStatus::Pending);
        assert_eq!(RunStatus::parse("SUPERSEDED"), RunStatus::Other);
        assert_eq!(RunStatus::parse("ASSIGNED"), RunStatus::Running);
    }
}
