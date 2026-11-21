use std::collections::{HashMap, VecDeque};
use std::fmt::Write;

use forage_core::platform::{
    Artifact, ArtifactDestination, DeploymentStates, Environment, PipelineRunStageState,
    ReleaseIntentState,
};

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
    pub trunks: Vec<GraphTrunk>,
    pub width: usize,
    pub height: usize,
    pub live: bool,
    pub has_logs: bool,
    pub plans: Vec<GraphPlan>,
}

pub struct GraphPlan {
    pub stage_id: String,
    pub output: String,
}

pub struct GraphTrunk {
    pub path: String,
    pub state: NodeState,
}

#[derive(Clone, Copy)]
struct EnvironmentOperation<'a> {
    row: &'a forage_core::platform::DestinationState,
    artifact: Option<&'a Artifact>,
    project: Option<&'a str>,
    state: NodeState,
    summary: &'static str,
    plan: bool,
    deploy: bool,
}

impl EnvironmentOperation<'_> {
    fn reference(&self) -> &str {
        self.artifact
            .map(|artifact| {
                artifact
                    .git_ref
                    .as_ref()
                    .and_then(|git| git.version.as_deref())
                    .filter(|version| !version.trim().is_empty())
                    .unwrap_or(&artifact.slug)
            })
            .unwrap_or_else(|| self.row.artifact_id.as_deref().unwrap_or("Unknown release"))
    }

    fn incoming(&self) -> bool {
        matches!(
            self.state,
            NodeState::Running
                | NodeState::Waiting
                | NodeState::Queued
                | NodeState::Next
                | NodeState::Blocked
        )
    }

    fn latest_at(&self) -> &str {
        self.row
            .completed_at
            .as_deref()
            .or(self.row.started_at.as_deref())
            .or(self.row.queued_at.as_deref())
            .unwrap_or("")
    }
}

struct EnvironmentBand {
    index: usize,
    total: usize,
    changing: usize,
    first: Option<usize>,
}

pub struct GraphNode {
    pub id: String,
    pub key: String,
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

impl GraphNode {
    pub fn card_height(&self) -> usize {
        if self.kind == "environment" { 104 } else { 168 }
    }
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
    /// Current environment state only; historical pipeline runs never determine these nodes.
    pub fn environments<'a>(
        org: &str,
        environments: &[Environment],
        current: &'a DeploymentStates,
        artifacts: impl IntoIterator<Item = (&'a str, &'a Artifact)>,
        intents: &[ReleaseIntentState],
    ) -> Self {
        use std::collections::BTreeMap;

        let mut graph = Self::default();
        let artifacts: HashMap<_, _> = artifacts
            .into_iter()
            .map(|(project, artifact)| (artifact.artifact_id.as_str(), (project, artifact)))
            .collect();
        let mut operation_by_release = HashMap::new();
        for intent in intents {
            let stages: HashMap<_, _> = intent
                .stages
                .iter()
                .map(|stage| (stage.stage_id.as_str(), stage.stage_type.as_str()))
                .collect();
            for stage in &intent.stages {
                for release_id in &stage.release_ids {
                    operation_by_release.insert(
                        release_id.as_str(),
                        (intent, Some(stage.stage_type.as_str())),
                    );
                }
            }
            for step in &intent.steps {
                let operation = step
                    .stage_id
                    .as_deref()
                    .and_then(|id| stages.get(id).copied());
                if operation.is_some() {
                    operation_by_release.insert(step.release_id.as_str(), (intent, operation));
                } else {
                    operation_by_release
                        .entry(step.release_id.as_str())
                        .or_insert((intent, None));
                }
            }
        }
        let mut destinations: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (_, artifact) in artifacts.values() {
            for destination in &artifact.destinations {
                destinations
                    .entry((destination.environment.as_str(), destination.name.as_str()))
                    .or_default();
            }
        }
        for destination in &current.destinations {
            destinations
                .entry((
                    destination.environment.as_str(),
                    destination.destination_name.as_str(),
                ))
                .or_default()
                .push(destination);
        }
        let mut ordered_environments: Vec<_> = environments.iter().collect();
        ordered_environments.sort_by_key(|environment| environment.sort_order);
        let mut environment_nodes = HashMap::new();
        for environment in ordered_environments
            .iter()
            .map(|environment| environment.name.as_str())
            .chain(destinations.keys().map(|(environment, _)| *environment))
        {
            if environment_nodes.contains_key(environment) {
                continue;
            }
            let index = graph.push(
                if environment.is_empty() {
                    "Unspecified environment".into()
                } else {
                    environment.into()
                },
                "Environment".into(),
                NodeState::Unknown,
                "No current destination state reported".into(),
                String::new(),
                format!("/orgs/{}/destinations", forage_ui::format::urlencode(org)),
            );
            graph.nodes[index].kind = "environment";
            graph.nodes[index].environment = environment.into();
            graph.nodes[index].key = format!("environment:{environment}");
            environment_nodes.insert(
                environment,
                EnvironmentBand {
                    index,
                    total: 0,
                    changing: 0,
                    first: None,
                },
            );
        }
        let priority = |state| match state {
            NodeState::Failed => 0,
            NodeState::Blocked => 1,
            NodeState::Running => 2,
            NodeState::Waiting => 3,
            NodeState::Queued => 4,
            NodeState::Next => 5,
            NodeState::Unknown => 6,
            NodeState::Cancelled => 7,
            NodeState::Superseded => 8,
            NodeState::Skipped => 9,
            NodeState::Done => 10,
        };
        let mut links = Vec::with_capacity(destinations.len());
        for ((environment, name), rows) in destinations {
            let mut primary: Option<EnvironmentOperation<'_>> = None;
            let mut completed: Option<EnvironmentOperation<'_>> = None;
            let mut incoming: Option<EnvironmentOperation<'_>> = None;
            let mut incoming_count = 0;
            let mut details = String::new();
            for row in rows {
                let execution = row
                    .release_id
                    .as_deref()
                    .and_then(|id| operation_by_release.get(id))
                    .filter(|(intent, _)| {
                        row.artifact_id
                            .as_deref()
                            .is_none_or(|id| id == intent.artifact_id)
                    });
                let intent = execution.map(|(intent, _)| *intent);
                let artifact_id = row
                    .artifact_id
                    .as_deref()
                    .or_else(|| intent.map(|intent| intent.artifact_id.as_str()));
                let artifact = artifact_id.and_then(|id| artifacts.get(id).copied());
                let mut state = NodeState::parse(row.status.as_deref().unwrap_or(""));
                if matches!(state, NodeState::Running | NodeState::Queued)
                    && row
                        .error_message
                        .as_deref()
                        .is_some_and(|reason| !reason.trim().is_empty())
                {
                    state = NodeState::Waiting;
                }
                let operation = execution.and_then(|(_, operation)| *operation);
                let operation = EnvironmentOperation {
                    row,
                    artifact: artifact.map(|(_, artifact)| artifact),
                    project: artifact
                        .map(|(project, _)| project)
                        .or_else(|| intent.map(|intent| intent.project.as_str())),
                    state,
                    plan: operation == Some("plan"),
                    deploy: operation == Some("deploy"),
                    summary: match operation {
                        Some("plan") if state == NodeState::Done => "Plan complete · not deployed",
                        Some("plan") => "Plan run · not deployed",
                        Some("deploy") if state == NodeState::Done => "Deploy complete",
                        Some("deploy") => "Deploy run",
                        Some(_) => "Current operation",
                        None => "Operation provenance unavailable",
                    },
                };
                if primary
                    .is_none_or(|previous| priority(operation.state) < priority(previous.state))
                {
                    primary = Some(operation);
                }
                if operation.state.terminal()
                    && completed.is_none_or(|previous| operation.latest_at() > previous.latest_at())
                {
                    completed = Some(operation);
                }
                if operation.incoming() {
                    incoming_count += 1;
                    if incoming
                        .is_none_or(|previous| priority(operation.state) < priority(previous.state))
                    {
                        incoming = Some(operation);
                    }
                }
                graph.live |= !operation.state.terminal();
                let _ = writeln!(
                    details,
                    "Execution: {} · {} · {}",
                    operation
                        .row
                        .release_id
                        .as_deref()
                        .unwrap_or("Unknown execution"),
                    operation.summary,
                    operation.state.label()
                );
                if let Some(artifact) = operation.artifact {
                    let _ = writeln!(
                        details,
                        "{} · {} · {}",
                        operation.project.unwrap_or("Unknown project"),
                        artifact.slug,
                        artifact.context.title
                    );
                } else {
                    let _ = writeln!(
                        details,
                        "Release provenance unavailable · {}",
                        operation.reference()
                    );
                }
                for (label, value) in [
                    ("Queued", &operation.row.queued_at),
                    ("Started", &operation.row.started_at),
                    ("Completed", &operation.row.completed_at),
                ] {
                    if let Some(value) = value {
                        let _ = writeln!(details, "{label}: {value}");
                    }
                }
                if let Some(reason) = &operation.row.error_message {
                    let _ = writeln!(details, "{reason}");
                }
                details.push('\n');
            }
            let summary = if let (Some(_), Some(completed)) = (incoming, completed) {
                if completed.plan && completed.state == NodeState::Done {
                    format!("Plan complete · not deployed: {}", completed.reference())
                } else if completed.deploy && completed.state == NodeState::Done {
                    format!(
                        "Current: {} · {}",
                        completed.reference(),
                        completed.state.label()
                    )
                } else {
                    format!(
                        "Last op: {} · {}",
                        completed.reference(),
                        completed.state.label()
                    )
                }
            } else {
                primary
                    .map(|operation| operation.summary.to_owned())
                    .unwrap_or_else(|| "Operation provenance unavailable".into())
            };
            let mut preview = if let Some(incoming) = incoming {
                format!(
                    "Incoming: {} · {}",
                    incoming.reference(),
                    incoming.state.label()
                )
            } else {
                primary
                    .map(|operation| {
                        operation
                            .row
                            .error_message
                            .as_deref()
                            .filter(|reason| !reason.trim().is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| {
                                format!(
                                    "Release: {} · {}",
                                    operation.project.unwrap_or("Unknown project"),
                                    operation.reference()
                                )
                            })
                    })
                    .unwrap_or_else(|| "Release provenance unavailable".into())
            };
            if incoming_count > 1 {
                let _ = write!(preview, " (+{})", incoming_count - 1);
            }
            let state = primary
                .map(|operation| operation.state)
                .unwrap_or(NodeState::Unknown);
            let reason = primary
                .map(|operation| operation.row.error_message.clone().unwrap_or_default())
                .unwrap_or_else(|| "No current destination state reported".into());
            let context = primary
                .and_then(|operation| {
                    operation
                        .artifact
                        .map(|artifact| (operation.project.unwrap_or("Unknown project"), artifact))
                })
                .map(|(project, artifact)| {
                    format!("{project} · {} · {}", artifact.slug, artifact.context.title)
                })
                .unwrap_or_else(|| "Release provenance unavailable".into());
            let href = primary
                .and_then(|operation| operation.artifact.zip(operation.project))
                .map(|(artifact, project)| {
                    super::release::release_href(org, project, &artifact.slug)
                })
                .unwrap_or_else(|| {
                    format!(
                        "/orgs/{}/destinations/detail?name={}",
                        forage_ui::format::urlencode(org),
                        forage_ui::format::urlencode(name)
                    )
                });
            let index = graph.push(name.into(), context, state, reason, details, href);
            graph.nodes[index].key = format!("target:{}:{environment}{name}", environment.len());
            graph.nodes[index].environment = environment.into();
            graph.nodes[index].summary = summary;
            graph.nodes[index].preview = preview;
            if let Some(operation) = primary {
                graph.nodes[index].timestamp = timestamp(
                    operation.row.queued_at.as_deref(),
                    operation.row.started_at.as_deref(),
                    operation.row.completed_at.as_deref(),
                );
            }
            let band = environment_nodes.get_mut(environment).unwrap();
            if band.total == 0 || priority(state) < priority(graph.nodes[band.index].state) {
                graph.nodes[band.index].state = state;
            }
            band.first.get_or_insert(index);
            band.total += 1;
            band.changing += usize::from(incoming_count > 0);
            links.push((band.index, index));
        }
        for band in environment_nodes.values() {
            if band.total > 0 {
                graph.nodes[band.index].reason.clear();
                graph.nodes[band.index].summary =
                    format!("{} destinations · {} changing", band.total, band.changing);
            }
        }
        graph.layout_environments(&links, &mut environment_nodes);
        graph
    }

    fn layout_environments(
        &mut self,
        links: &[(usize, usize)],
        bands: &mut HashMap<&str, EnvironmentBand>,
    ) {
        let mut top = 32;
        for root in 0..bands.len() {
            let band = bands
                .get_mut(self.nodes[root].environment.as_str())
                .unwrap();
            let columns = band.total.clamp(1, 4);
            let rows = band.total.max(1).div_ceil(4);
            let width = columns * 336 - 64;
            self.nodes[root].x = 32 + (width - 272) / 2;
            self.nodes[root].y = top;
            if let Some(first) = band.first {
                for (offset, node) in self.nodes[first..first + band.total].iter_mut().enumerate() {
                    node.x = 32 + offset % 4 * 336;
                    node.y = top + 184 + offset / 4 * 232;
                }
                let center = (self.nodes[root].x + 136) as f64;
                let first_bus = (top + 144) as f64;
                let mut path = rounded_path(&[(center, (top + 104) as f64), (center, first_bus)]);
                for row in 0..rows {
                    let count = (band.total - row * 4).min(4);
                    let right = (168 + (count - 1) * 336) as f64;
                    let bus = (top + 144 + row * 232) as f64;
                    if row == 0 {
                        let _ = write!(path, " M 168.0 {bus:.1} L {right:.1} {bus:.1}");
                    } else {
                        path.push_str(&rounded_path(&[
                            (168.0, first_bus),
                            (16.0, first_bus),
                            (16.0, bus),
                            (right, bus),
                        ]));
                    }
                }
                self.trunks.push(GraphTrunk {
                    path,
                    state: self.nodes[root].state,
                });
            }
            self.width = self.width.max(width + 64);
            top += 184 + (rows - 1) * 232 + 168 + 80;
        }
        self.width = self.width.max(336);
        self.height = top;
        self.edges = links
            .iter()
            .map(|&(from, to)| {
                let source = &self.nodes[from];
                let target = &self.nodes[to];
                let start_x = (target.x + 136) as f64;
                let start_y = (target.y - 40) as f64;
                let end_x = start_x;
                let end_y = target.y as f64;
                GraphEdge {
                    from: source.id.clone(),
                    to: target.id.clone(),
                    path: rounded_path(&[(start_x, start_y), (end_x, end_y)]),
                    state: target.state,
                    start_x,
                    start_y,
                    end_x,
                    end_y,
                }
            })
            .collect();
    }

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
            key: String::new(),
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

    pub fn build<'a>(
        intent: Option<&ReleaseIntentState>,
        policies: &[PolicyCheck],
        configured: &[ArtifactDestination],
        plan_outputs: impl IntoIterator<Item = (&'a str, &'a str)>,
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
            graph.layout(&[], &[], true);
            return graph;
        };
        graph.has_logs = true;
        let mut links = Vec::new();
        let mut state_dependencies = Vec::new();
        let mut entries = HashMap::with_capacity(intent.stages.len());
        let mut exits = HashMap::with_capacity(intent.stages.len());
        let plan_outputs: HashMap<_, _> = plan_outputs.into_iter().collect();
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
            let plan_index = (stage.stage_type == "plan").then(|| {
                let output = plan_outputs
                    .get(stage.stage_id.as_str())
                    .copied()
                    .unwrap_or_default();
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
                    stage_id: stage.stage_id.clone(),
                    output: output.to_owned(),
                });
                plan_index
            });
            graph.nodes[index].plan_index = plan_index;
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
        graph.layout(&links, &state_dependencies, true);
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

    fn layout(
        &mut self,
        links: &[(usize, usize)],
        state_dependencies: &[(usize, usize)],
        infer_dependencies: bool,
    ) {
        let mut outgoing = vec![Vec::new(); self.nodes.len()];
        let mut incoming = vec![0usize; self.nodes.len()];
        let mut predecessors = vec![Vec::new(); self.nodes.len()];
        let mut dependencies = vec![Vec::new(); self.nodes.len()];
        for (edge, &(from, to)) in links.iter().enumerate() {
            outgoing[from].push((to, edge));
            predecessors[to].push((from, edge));
            incoming[to] += 1;
            if infer_dependencies {
                dependencies[to].push(from);
            }
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
                    state: if !infer_dependencies {
                        b.state
                    } else if a.state == NodeState::Done && b.state == NodeState::Done {
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
