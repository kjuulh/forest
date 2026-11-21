use std::collections::{HashMap, VecDeque};
use std::fmt::Write;

use forage_core::platform::{ArtifactDestination, PipelineRunStageState, ReleaseIntentState};

use super::release::PolicyCheck;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    Running,
    Next,
    Queued,
    Waiting,
    Blocked,
    Failed,
    Done,
    Skipped,
    Cancelled,
    Superseded,
    Unknown,
}

impl NodeState {
    pub fn class(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Next => "next",
            Self::Queued => "queued",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Done => "done",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
            Self::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running now",
            Self::Next => "Next",
            Self::Queued => "Queued",
            Self::Waiting => "Waiting",
            Self::Blocked => "Blocked",
            Self::Failed => "Failed",
            Self::Done => "Done",
            Self::Skipped => "Skipped",
            Self::Cancelled => "Cancelled",
            Self::Superseded => "Superseded",
            Self::Unknown => "Unknown",
        }
    }

    fn parse(status: &str) -> Self {
        match status {
            "RUNNING" | "ASSIGNED" | "ACTIVE" => Self::Running,
            "PENDING" => Self::Next,
            "QUEUED" => Self::Queued,
            "WAITING" | "AWAITING_APPROVAL" | "AWAITINGAPPROVAL" => Self::Waiting,
            "BLOCKED" => Self::Blocked,
            "FAILED" | "TIMED_OUT" => Self::Failed,
            "SUCCEEDED" | "DONE" => Self::Done,
            "SKIPPED" => Self::Skipped,
            "CANCELLED" => Self::Cancelled,
            "SUPERSEDED" => Self::Superseded,
            _ => Self::Unknown,
        }
    }

    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Skipped | Self::Failed | Self::Cancelled | Self::Superseded
        )
    }
}

#[derive(Default)]
pub struct ReleaseGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub width: usize,
    pub height: usize,
    pub live: bool,
    pub has_logs: bool,
    pub plans: Vec<GraphPlan>,
    pub queue: Vec<usize>,
}

pub struct GraphPlan {
    pub output: String,
}

pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub context: String,
    pub state: NodeState,
    pub reason: String,
    pub details: String,
    pub href: String,
    pub kind: &'static str,
    pub environment: String,
    pub summary: String,
    pub timestamp: String,
    pub preview: String,
    pub plan_index: Option<usize>,
    pub x: usize,
    pub y: usize,
}

pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub path: String,
    pub state: NodeState,
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
}

fn timing(
    queued: Option<&str>,
    started: Option<&str>,
    completed: Option<&str>,
    duration: Option<i64>,
) -> String {
    let mut parts = Vec::with_capacity(4);
    for (label, value) in [
        ("Queued", queued),
        ("Started", started),
        ("Completed", completed),
    ] {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            parts.push(format!("{label}: {value}"));
        }
    }
    if let Some(duration) = duration {
        parts.push(format!("Duration: {duration}s"));
    } else if let (Some(start), Some(end)) = (started, completed) {
        if let (Ok(start), Ok(end)) = (
            chrono::DateTime::parse_from_rfc3339(start),
            chrono::DateTime::parse_from_rfc3339(end),
        ) {
            parts.push(format!("Duration: {}s", (end - start).num_seconds().max(0)));
        }
    }
    parts.join(" · ")
}

fn stage_state(stage: &PipelineRunStageState) -> NodeState {
    let state = NodeState::parse(&stage.status);
    let reason = stage.error_message.as_deref().unwrap_or_default().trim();
    if reason.starts_with("superseded") && state.terminal() {
        NodeState::Superseded
    } else if !state.terminal() && reason.starts_with("stale:") {
        NodeState::Blocked
    } else if !state.terminal()
        && (stage.stage_type == "wait"
            || stage.stage_type == "gate"
            || !stage.gate_waiting_on.is_empty()
            || (stage.stage_type == "plan" && !reason.is_empty()))
        && state != NodeState::Next
    {
        NodeState::Waiting
    } else {
        state
    }
}

fn short_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

fn timestamp(queued: Option<&str>, started: Option<&str>, completed: Option<&str>) -> String {
    let (label, value) = if let Some(value) = completed {
        ("Finished", value)
    } else if let Some(value) = started {
        ("Started", value)
    } else if let Some(value) = queued {
        ("Queued", value)
    } else {
        return String::new();
    };
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| {
            format!(
                "{label} {} UTC",
                value.with_timezone(&chrono::Utc).format("%H:%M")
            )
        })
        .unwrap_or_default()
}

fn elapsed(started: Option<&str>, completed: Option<&str>) -> Option<i64> {
    Some(
        (chrono::DateTime::parse_from_rfc3339(completed?).ok()?
            - chrono::DateTime::parse_from_rfc3339(started?).ok()?)
        .num_seconds()
        .max(0),
    )
}

impl ReleaseGraph {
    fn push(
        &mut self,
        label: String,
        context: String,
        state: NodeState,
        reason: String,
        details: String,
        href: String,
    ) -> usize {
        let index = self.nodes.len();
        self.nodes.push(GraphNode {
            id: format!("graph-node-{index}"),
            label,
            context,
            state,
            reason,
            details,
            href,
            kind: "destination",
            environment: String::new(),
            summary: String::new(),
            timestamp: String::new(),
            preview: String::new(),
            plan_index: None,
            x: 0,
            y: 0,
        });
        index
    }

    pub fn build(
        intent: Option<&ReleaseIntentState>,
        policies: &[PolicyCheck],
        configured: &[ArtifactDestination],
        stage_views: &[forage_ui::release::Stage],
    ) -> Self {
        let mut graph = Self::default();
        let Some(intent) = intent else {
            for dest in configured {
                let index = graph.push(
                    dest.name.clone(),
                    dest.environment.clone(),
                    NodeState::Waiting,
                    "Release has not been triggered".into(),
                    String::new(),
                    "#destinations".into(),
                );
                graph.nodes[index].environment = dest.environment.clone();
                graph.nodes[index].summary = "Configured destination".into();
            }
            graph.layout(&[], &[]);
            return graph;
        };
        graph.has_logs = true;
        let mut links = Vec::new();
        let mut queue_candidates = Vec::new();
        let mut state_dependencies = Vec::new();
        let mut entries = HashMap::with_capacity(intent.stages.len());
        let mut exits = HashMap::with_capacity(intent.stages.len());
        let stage_views: HashMap<_, _> = stage_views
            .iter()
            .map(|view| (view.id.as_str(), view))
            .collect();
        let stage_for_release: HashMap<_, _> = intent
            .stages
            .iter()
            .flat_map(|stage| {
                stage
                    .release_ids
                    .iter()
                    .map(move |release| (release.as_str(), stage.stage_id.as_str()))
            })
            .collect();
        let mut steps_by_stage: HashMap<_, Vec<_>> = intent
            .stages
            .iter()
            .map(|stage| (stage.stage_id.as_str(), Vec::new()))
            .collect();
        let mut unassociated = Vec::new();
        for step in &intent.steps {
            let owner = step
                .stage_id
                .as_deref()
                .or_else(|| stage_for_release.get(step.release_id.as_str()).copied());
            if let Some(steps) = owner.and_then(|owner| steps_by_stage.get_mut(owner)) {
                steps.push(step);
            } else {
                unassociated.push(step);
            }
        }
        for stage in &intent.stages {
            let state = stage_state(stage);
            graph.live |= !state.terminal();
            let anchor = forage_core::integrations::slack_release::anchor_id(&stage.stage_id);
            let href = format!("#stage-{anchor}");
            let mut reason = stage.error_message.clone().unwrap_or_default();
            if reason.is_empty() && !stage.gate_waiting_on.is_empty() {
                reason = format!("Waiting for {}", stage.gate_waiting_on.join(", "));
            }
            if reason.is_empty() && state == NodeState::Waiting {
                if let Some(until) = &stage.wait_until {
                    reason = format!("Waiting until {until}");
                }
            }
            let mut details = timing(
                stage.queued_at.as_deref(),
                stage.started_at.as_deref(),
                stage.completed_at.as_deref(),
                stage.duration_seconds,
            );
            if let Some(deadline) = &stage.gate_deadline {
                details.push_str(&format!(" · Deadline: {deadline}"));
            }
            let index = graph.push(
                stage.stage_id.clone(),
                format!(
                    "{}{}",
                    stage.stage_type,
                    stage
                        .environment
                        .as_ref()
                        .map(|e| format!(" · {e}"))
                        .unwrap_or_default()
                ),
                state,
                reason,
                details,
                href.clone(),
            );
            graph.nodes[index].kind = match stage.stage_type.as_str() {
                "plan" => "plan",
                "deploy" => "deploy",
                "wait" => "wait",
                "gate" => "gate",
                _ => "policy",
            };
            graph.nodes[index].environment = stage.environment.clone().unwrap_or_default();
            graph.nodes[index].timestamp = timestamp(
                stage.queued_at.as_deref(),
                stage.started_at.as_deref(),
                stage.completed_at.as_deref(),
            );
            let plan_index = stage_views
                .get(stage.stage_id.as_str())
                .and_then(|view| view.plan_output.as_ref())
                .map(|output| {
                    let plan_index = graph.plans.len();
                    graph.nodes[index].preview = output
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or_default()
                        .trim()
                        .chars()
                        .take(80)
                        .collect();
                    graph.plans.push(GraphPlan {
                        output: output.clone(),
                    });
                    plan_index
                });
            graph.nodes[index].plan_index = plan_index;
            if matches!(
                state,
                NodeState::Running | NodeState::Queued | NodeState::Waiting | NodeState::Blocked
            ) {
                queue_candidates.push(index);
            }
            entries.insert(stage.stage_id.as_str(), index);
            let mut leaves = Vec::new();
            for step in &steps_by_stage[stage.stage_id.as_str()] {
                let mut step_state = NodeState::parse(&step.status);
                if matches!(step_state, NodeState::Running | NodeState::Queued)
                    && step
                        .error_message
                        .as_deref()
                        .is_some_and(|r| !r.trim().is_empty())
                {
                    step_state = NodeState::Waiting;
                }
                if step
                    .error_message
                    .as_deref()
                    .is_some_and(|r| r.starts_with("superseded"))
                    && step_state.terminal()
                {
                    step_state = NodeState::Superseded;
                }
                graph.live |= !step_state.terminal();
                let child = graph.push(
                    step.destination_name.clone(),
                    format!("{} · {}", stage.stage_id, step.environment),
                    step_state,
                    step.error_message.clone().unwrap_or_default(),
                    timing(
                        step.queued_at.as_deref(),
                        step.started_at.as_deref(),
                        step.completed_at.as_deref(),
                        None,
                    ),
                    href.clone(),
                );
                graph.nodes[child].kind = graph.nodes[index].kind;
                graph.nodes[child].environment = step.environment.clone();
                graph.nodes[child].summary =
                    elapsed(step.started_at.as_deref(), step.completed_at.as_deref())
                        .map(|seconds| format!("{} · {}", stage.stage_id, short_duration(seconds)))
                        .unwrap_or_else(|| stage.stage_id.clone());
                graph.nodes[child].timestamp = timestamp(
                    step.queued_at.as_deref(),
                    step.started_at.as_deref(),
                    step.completed_at.as_deref(),
                );
                graph.nodes[child].plan_index = plan_index;
                graph.nodes[child].preview = graph.nodes[index].preview.clone();
                if matches!(
                    step_state,
                    NodeState::Running
                        | NodeState::Queued
                        | NodeState::Waiting
                        | NodeState::Blocked
                ) {
                    queue_candidates.push(child);
                }
                links.push((index, child));
                leaves.push(child);
            }
            if leaves.is_empty() && matches!(stage.stage_type.as_str(), "deploy" | "plan") {
                for dest in configured
                    .iter()
                    .filter(|dest| stage.environment.as_deref() == Some(dest.environment.as_str()))
                {
                    let child = graph.push(
                        dest.name.clone(),
                        format!("{} · {}", stage.stage_id, dest.environment),
                        if state.terminal() {
                            NodeState::Unknown
                        } else {
                            NodeState::Waiting
                        },
                        "No destination run reported for this stage".into(),
                        String::new(),
                        href.clone(),
                    );
                    graph.nodes[child].kind = graph.nodes[index].kind;
                    graph.nodes[child].environment = dest.environment.clone();
                    graph.nodes[child].summary = stage.stage_id.clone();
                    graph.nodes[child].plan_index = plan_index;
                    links.push((index, child));
                    // Unreported destination state must not gate a completed stage.
                }
            }
            let completed = leaves
                .iter()
                .filter(|&&leaf| graph.nodes[leaf].state == NodeState::Done)
                .count();
            let total = if leaves.is_empty() {
                configured
                    .iter()
                    .filter(|dest| stage.environment.as_deref() == Some(dest.environment.as_str()))
                    .count()
            } else {
                leaves.len()
            };
            graph.nodes[index].summary = if total > 0 {
                format!("{completed} / {total} destination runs")
            } else if stage.stage_type == "wait" {
                "Soak timer".into()
            } else {
                "Pipeline stage".into()
            };
            if let Some(duration) = stage
                .duration_seconds
                .or_else(|| elapsed(stage.started_at.as_deref(), stage.completed_at.as_deref()))
            {
                write!(
                    graph.nodes[index].summary,
                    " · {}",
                    short_duration(duration)
                )
                .unwrap();
            }
            if leaves.is_empty() {
                leaves.push(index);
            }
            if stage.stage_type == "plan"
                && (stage.approval_status.is_some() || stage.auto_approve == Some(false))
            {
                let approval = stage.approval_status.as_deref().unwrap_or_default();
                let approval_state = match approval {
                    "APPROVED" | "AUTO_APPROVED" | "AUTOAPPROVED" => NodeState::Done,
                    "REJECTED" => NodeState::Failed,
                    "AWAITING_APPROVAL" | "AWAITINGAPPROVAL" => {
                        if state == NodeState::Blocked {
                            NodeState::Blocked
                        } else {
                            NodeState::Waiting
                        }
                    }
                    _ if state.terminal() => state,
                    _ => NodeState::Waiting,
                };
                if matches!(approval, "AWAITING_APPROVAL" | "AWAITINGAPPROVAL")
                    && state != NodeState::Blocked
                {
                    graph.nodes[index].state = NodeState::Done;
                }
                let gate = graph.push(
                    "Plan approval".into(),
                    stage.environment.clone().unwrap_or_default(),
                    approval_state,
                    if graph.nodes[index].reason.is_empty() && approval_state == NodeState::Waiting
                    {
                        "Waiting for plan approval".into()
                    } else {
                        graph.nodes[index].reason.clone()
                    },
                    String::new(),
                    href,
                );
                graph.nodes[gate].kind = "approval";
                graph.nodes[gate].environment = stage.environment.clone().unwrap_or_default();
                graph.nodes[gate].summary = if stage.auto_approve == Some(true) {
                    "Automatic plan approval"
                } else {
                    "Manual plan gate"
                }
                .into();
                graph.nodes[gate].plan_index = plan_index;
                graph.nodes[gate].preview = graph.nodes[index].preview.clone();
                if matches!(approval, "AWAITING_APPROVAL" | "AWAITINGAPPROVAL") {
                    queue_candidates.push(gate);
                }
                links.extend(leaves.iter().map(|&leaf| (leaf, gate)));
                leaves = vec![gate];
            }
            exits.insert(stage.stage_id.as_str(), leaves);
        }
        for stage in &intent.stages {
            let index = entries[stage.stage_id.as_str()];
            let mut parents = Vec::new();
            for dep in &stage.depends_on {
                if let Some(leaves) = exits.get(dep.as_str()) {
                    parents.extend_from_slice(leaves);
                    state_dependencies.push((entries[dep.as_str()], index));
                    state_dependencies.extend(leaves.iter().map(|&leaf| (leaf, index)));
                } else if !graph.nodes[index].state.terminal() {
                    graph.nodes[index].state = NodeState::Blocked;
                    graph.nodes[index].reason = format!("Dependency {dep} was not reported");
                }
            }
            let mut admission = Vec::new();
            if stage.stage_type == "deploy" && !graph.nodes[index].state.terminal() {
                for policy in policies
                    .iter()
                    .filter(|p| stage.environment.as_deref() == Some(p.target_environment.as_str()))
                {
                    let gate = graph.push(
                        policy.name.clone(),
                        format!(
                            "{} · {} · {}",
                            policy.policy_type, policy.target_environment, stage.stage_id
                        ),
                        if policy.passed {
                            NodeState::Done
                        } else {
                            NodeState::Blocked
                        },
                        policy.reason.clone(),
                        "Current policy evaluation".into(),
                        "#policies".into(),
                    );
                    graph.nodes[gate].kind = "policy";
                    graph.nodes[gate].environment = policy.target_environment.clone();
                    graph.nodes[gate].summary = policy.policy_type.replace('_', " ");
                    admission.push(gate);
                    links.extend(parents.iter().map(|&p| (p, gate)));
                    links.push((gate, index));
                }
            }
            if admission.is_empty() {
                links.extend(parents.iter().map(|&p| (p, index)));
            }
        }
        // Policy evaluations describe now, not a past run's admission decision.
        for policy in policies.iter().filter(|p| {
            !intent.stages.iter().any(|s| {
                s.stage_type == "deploy"
                    && !stage_state(s).terminal()
                    && s.environment.as_deref() == Some(p.target_environment.as_str())
            })
        }) {
            let index = graph.push(
                policy.name.clone(),
                format!("{} · {}", policy.policy_type, policy.target_environment),
                if policy.passed {
                    NodeState::Done
                } else {
                    NodeState::Blocked
                },
                policy.reason.clone(),
                "Current policy evaluation".into(),
                "#policies".into(),
            );
            graph.nodes[index].kind = "policy";
            graph.nodes[index].environment = policy.target_environment.clone();
            graph.nodes[index].summary = policy.policy_type.replace('_', " ");
        }
        for step in unassociated {
            let state = NodeState::parse(&step.status);
            graph.live |= !state.terminal();
            let index = graph.push(
                step.destination_name.clone(),
                step.environment.clone(),
                state,
                step.error_message.clone().unwrap_or_default(),
                timing(
                    step.queued_at.as_deref(),
                    step.started_at.as_deref(),
                    step.completed_at.as_deref(),
                    None,
                ),
                "#logs".into(),
            );
            graph.nodes[index].environment = step.environment.clone();
            graph.nodes[index].summary = "Destination run".into();
            graph.nodes[index].timestamp = timestamp(
                step.queued_at.as_deref(),
                step.started_at.as_deref(),
                step.completed_at.as_deref(),
            );
            if matches!(
                state,
                NodeState::Running | NodeState::Queued | NodeState::Waiting | NodeState::Blocked
            ) {
                queue_candidates.push(index);
            }
        }
        if graph.nodes.is_empty() {
            for dest in configured {
                let index = graph.push(
                    dest.name.clone(),
                    dest.environment.clone(),
                    NodeState::Waiting,
                    "No destination run reported for this release".into(),
                    String::new(),
                    "#destinations".into(),
                );
                graph.nodes[index].environment = dest.environment.clone();
                graph.nodes[index].summary = "Configured destination".into();
            }
        }
        graph.layout(&links, &state_dependencies);
        graph.queue = queue_candidates
            .into_iter()
            .filter(|&index| !graph.nodes[index].state.terminal())
            .collect();
        for (index, node) in graph.nodes.iter().enumerate() {
            if node.state == NodeState::Next {
                graph.queue.push(index);
            }
        }
        // Future pending stages are not yet in the scheduler's queue.
        for stage in &intent.stages {
            let index = entries[stage.stage_id.as_str()];
            if stage.status == "PENDING"
                && graph.nodes[index].state == NodeState::Blocked
                && stage.depends_on.iter().all(|dep| {
                    entries
                        .get(dep.as_str())
                        .is_some_and(|&entry| graph.nodes[entry].state == NodeState::Done)
                        && exits.get(dep.as_str()).is_some_and(|leaves| {
                            leaves
                                .iter()
                                .all(|&leaf| graph.nodes[leaf].state == NodeState::Done)
                        })
                })
                && !graph.queue.contains(&index)
            {
                graph.queue.push(index);
            }
        }
        graph
    }

    fn resolve_pending(&mut self, index: usize, dependencies: &[usize]) {
        if self.nodes[index].state != NodeState::Next {
            return;
        }
        if let Some(&blocked) = dependencies.iter().find(|&&from| {
            matches!(
                self.nodes[from].state,
                NodeState::Failed
                    | NodeState::Blocked
                    | NodeState::Cancelled
                    | NodeState::Superseded
                    | NodeState::Skipped
                    | NodeState::Unknown
            )
        }) {
            self.nodes[index].state = NodeState::Blocked;
            self.nodes[index].reason = format!(
                "Blocked by {} ({})",
                self.nodes[blocked].label,
                self.nodes[blocked].state.label()
            );
        } else if let Some(&waiting) = dependencies
            .iter()
            .find(|&&from| self.nodes[from].state != NodeState::Done)
        {
            self.nodes[index].state = NodeState::Waiting;
            self.nodes[index].reason = format!(
                "Waiting for {} ({})",
                self.nodes[waiting].label,
                self.nodes[waiting].state.label()
            );
        }
    }

    fn layout(&mut self, links: &[(usize, usize)], state_dependencies: &[(usize, usize)]) {
        let mut outgoing = vec![Vec::new(); self.nodes.len()];
        let mut incoming = vec![0usize; self.nodes.len()];
        let mut predecessors = vec![Vec::new(); self.nodes.len()];
        let mut dependencies = vec![Vec::new(); self.nodes.len()];
        for (edge, &(from, to)) in links.iter().enumerate() {
            outgoing[from].push((to, edge));
            predecessors[to].push((from, edge));
            incoming[to] += 1;
            dependencies[to].push(from);
        }
        for &(from, to) in state_dependencies {
            if !dependencies[to].contains(&from) {
                dependencies[to].push(from);
            }
        }
        let mut depths = vec![0usize; self.nodes.len()];
        let mut queue: VecDeque<_> = incoming
            .iter()
            .enumerate()
            .filter(|(_, count)| **count == 0)
            .map(|(index, _)| index)
            .collect();
        while let Some(from) = queue.pop_front() {
            self.resolve_pending(from, &dependencies[from]);
            for &(to, _) in &outgoing[from] {
                depths[to] = depths[to].max(depths[from] + 1);
                incoming[to] -= 1;
                if incoming[to] == 0 {
                    queue.push_back(to);
                }
            }
        }
        let columns = depths.iter().copied().max().unwrap_or(0) + 1;
        let mut counts = vec![0usize; columns];
        for &depth in &depths {
            counts[depth] += 1;
        }
        let max_rows = counts.iter().copied().max().unwrap_or(0);
        let long_edges = links
            .iter()
            .filter(|&&(from, to)| depths[to] != depths[from] + 1)
            .count();
        let top = 32 + long_edges * 16;
        let mut rows = vec![0usize; columns];
        for (index, node) in self.nodes.iter_mut().enumerate() {
            if incoming[index] > 0 {
                node.state = NodeState::Blocked;
                node.reason = "Cyclic pipeline dependency".into();
            }
            let column = depths[index];
            node.x = 32 + column * 336;
            node.y = top + (max_rows - counts[column]) * 102 + rows[column] * 204;
            rows[column] += 1;
            self.width = self.width.max(node.x + 272 + 32);
            self.height = self.height.max(node.y + 168 + 32);
        }
        let mut source_ports = vec![0.0; links.len()];
        let mut target_ports = vec![0.0; links.len()];
        for (index, edges) in outgoing.iter_mut().enumerate() {
            edges.sort_by_key(|&(to, _)| self.nodes[to].y);
            for (port, &(_, edge)) in edges.iter().enumerate() {
                source_ports[edge] = self.nodes[index].y as f64
                    + (port + 1) as f64 * 168.0 / (edges.len() + 1) as f64;
            }
        }
        for (index, edges) in predecessors.iter_mut().enumerate() {
            edges.sort_by_key(|&(from, _)| self.nodes[from].y);
            for (port, &(_, edge)) in edges.iter().enumerate() {
                target_ports[edge] = self.nodes[index].y as f64
                    + (port + 1) as f64 * 168.0 / (edges.len() + 1) as f64;
            }
        }
        let mut lane = 0;
        self.edges = links
            .iter()
            .enumerate()
            .map(|(edge, &(from, to))| {
                let a = &self.nodes[from];
                let b = &self.nodes[to];
                let x = (a.x + 272) as f64;
                let y = source_ports[edge];
                let end_x = b.x as f64;
                let end_y = target_ports[edge];
                let path = if depths[to] == depths[from] + 1 {
                    let middle = (x + end_x) / 2.0;
                    rounded_path(&[(x, y), (middle, y), (middle, end_y), (end_x, end_y)])
                } else {
                    let route_y = 16.0 + lane as f64 * 16.0;
                    lane += 1;
                    rounded_path(&[
                        (x, y),
                        (x + 20.0, y),
                        (x + 20.0, route_y),
                        (end_x - 20.0, route_y),
                        (end_x - 20.0, end_y),
                        (end_x, end_y),
                    ])
                };
                GraphEdge {
                    from: a.id.clone(),
                    to: b.id.clone(),
                    path,
                    state: if a.state == NodeState::Done && b.state == NodeState::Done {
                        NodeState::Done
                    } else if a.state == NodeState::Running {
                        NodeState::Running
                    } else {
                        NodeState::Waiting
                    },
                    start_x: x,
                    start_y: y,
                    end_x,
                    end_y,
                }
            })
            .collect();
    }
}

fn rounded_path(points: &[(f64, f64)]) -> String {
    let mut path = String::with_capacity(points.len() * 48);
    write!(path, "M {:.1} {:.1}", points[0].0, points[0].1).unwrap();
    for corner in points.windows(3) {
        let (before, at, after) = (corner[0], corner[1], corner[2]);
        let in_length = (at.0 - before.0).abs() + (at.1 - before.1).abs();
        let out_length = (after.0 - at.0).abs() + (after.1 - at.1).abs();
        if in_length == 0.0 || out_length == 0.0 {
            continue;
        }
        let radius = 10.0_f64.min(in_length / 2.0).min(out_length / 2.0);
        let start = (
            at.0 - (at.0 - before.0) * radius / in_length,
            at.1 - (at.1 - before.1) * radius / in_length,
        );
        let end = (
            at.0 + (after.0 - at.0) * radius / out_length,
            at.1 + (after.1 - at.1) * radius / out_length,
        );
        write!(
            path,
            " L {:.1} {:.1} Q {:.1} {:.1} {:.1} {:.1}",
            start.0, start.1, at.0, at.1, end.0, end.1
        )
        .unwrap();
    }
    let last = points[points.len() - 1];
    write!(path, " L {:.1} {:.1}", last.0, last.1).unwrap();
    path
}
