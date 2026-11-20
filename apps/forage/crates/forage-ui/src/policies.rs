#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyKind {
    #[default]
    SoakTime,
    BranchRestriction,
    Approval,
    SupersedePending,
}

impl PolicyKind {
    pub const ALL: [PolicyKind; 4] = [
        PolicyKind::SoakTime,
        PolicyKind::BranchRestriction,
        PolicyKind::Approval,
        PolicyKind::SupersedePending,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            PolicyKind::SoakTime => "soak_time",
            PolicyKind::BranchRestriction => "branch_restriction",
            PolicyKind::Approval => "approval",
            PolicyKind::SupersedePending => "supersede_pending",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PolicyKind::SoakTime => "Soak Time",
            PolicyKind::BranchRestriction => "Branch Restriction",
            PolicyKind::Approval => "Approval Required",
            PolicyKind::SupersedePending => "Supersede Pending",
        }
    }

    pub fn chip_classes(self) -> &'static str {
        match self {
            PolicyKind::SoakTime => "bg-indigo-100 text-indigo-700",
            PolicyKind::BranchRestriction => "bg-orange-100 text-orange-700",
            PolicyKind::Approval => "bg-emerald-100 text-emerald-700",
            PolicyKind::SupersedePending => "bg-sky-100 text-sky-700",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            PolicyKind::SoakTime => {
                "Require an artifact to succeed in a source environment for a duration before deploying to target."
            }
            PolicyKind::BranchRestriction => {
                "Only allow deployments to the target environment from a specific branch pattern."
            }
            PolicyKind::Approval => {
                "Require one or more team members to approve before deploying to the target environment."
            }
            PolicyKind::SupersedePending => {
                "When releases queue up for a destination, deploy the newest and mark the ones it overtook superseded instead of deploying each in turn."
            }
        }
    }

    pub fn fields_id(self) -> &'static str {
        match self {
            PolicyKind::SoakTime => "soak-time-fields",
            PolicyKind::BranchRestriction => "branch-restriction-fields",
            PolicyKind::Approval => "approval-fields",
            PolicyKind::SupersedePending => "supersede-pending-fields",
        }
    }
}
