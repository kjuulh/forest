use std::collections::BTreeMap;
use std::fmt::Display;

use anyhow::Context;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    cli::output::OutputFormat,
    grpc::{
        GetProjectsQuery, GrpcClientState,
        release_watch::{ReleaseProgress, WatchEnd, WatchMode},
    },
    models::{project::Project, release_annotation::ReleaseAnnotation},
    state::State,
};

/// Render full detail (header, stages, destinations, plan output, deploy
/// logs) for a single release. Mirrors what the web UI shows; uses the
/// same persisted log stream that `release create` follows live.
///
/// By default a snapshot: it prints what has been persisted and exits, also
/// for a release still in flight or parked awaiting approval. For a parked
/// plan stage it ends with the command that approves it, so read the plan and
/// then run that line. `--follow` streams until the release is terminal.
#[derive(clap::Parser)]
pub struct ShowCommand {
    /// Release slug or release-intent UUID. Omit for an interactive picker.
    #[arg()]
    target: Option<String>,

    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Limit rendering to a single stage by id.
    #[arg(long)]
    stage: Option<String>,

    /// Skip header/stage/destination sections; print only plan output
    /// and deploy log blocks. Useful for `... --logs-only > release.log`.
    #[arg(long)]
    logs_only: bool,

    /// Keep streaming until the release is terminal instead of exiting once
    /// the persisted history is printed. On a release parked awaiting approval
    /// this waits until someone approves or rejects it.
    #[arg(long)]
    follow: bool,
}

impl ShowCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let grpc = state.grpc_client();

        let resolved = match self.target.as_deref() {
            Some(target) => resolve_target(state, target, self.organisation.as_deref()).await?,
            None => {
                pick_release_interactive(
                    state,
                    self.organisation.as_deref(),
                    self.project.as_deref(),
                )
                .await?
            }
        };

        let ResolvedRelease {
            annotation,
            project,
            intent_id,
            intent_state,
        } = resolved;

        let stages_filter: Option<&str> = self.stage.as_deref();

        // Fetch plan output for each plan stage in scope.
        let mut plan_outputs: BTreeMap<String, forest_grpc_interface::GetPlanOutputResponse> =
            BTreeMap::new();
        for stage in intent_state
            .stages
            .iter()
            .filter(|s| stages_filter.map(|sf| s.stage_id == sf).unwrap_or(true))
        {
            if !is_plan_stage(stage.stage_type) {
                continue;
            }
            match grpc.get_plan_output(intent_id, &stage.stage_id).await {
                Ok(plan) => {
                    plan_outputs.insert(stage.stage_id.clone(), plan);
                }
                Err(e) => {
                    tracing::warn!(stage = %stage.stage_id, "could not fetch plan output: {e:#}");
                }
            }
        }

        // Signals say what a destination's status cannot: flux@1 reports
        // `reconcile` DEGRADED when the push landed but nothing told Flux to
        // sync, which a bare SUCCEEDED would hide. A server without the
        // signal service just shows none.
        let signals = match grpc.list_signals(intent_id).await {
            Ok(signals) => signals,
            Err(e) => {
                tracing::warn!("could not fetch release signals: {e:#}");
                Vec::new()
            }
        };

        let mut output = ShowOutput::new(
            &annotation,
            &project,
            &intent_id,
            &intent_state,
            &plan_outputs,
            stages_filter,
        );
        output.attach_signals(&signals);

        let format = state.config.format;
        let json_mode = matches!(format, OutputFormat::Json);

        // Render header eagerly so the user sees structure before the
        // potentially long log replay starts.
        if !json_mode && !self.logs_only {
            print!("{}", output.render_header());
        }

        // Collect logs by destination via WaitRelease, the stream `release
        // create` follows live. By default only what is persisted: the stream
        // stays open until the release is terminal, and a plan parked on its
        // approval never gets there until someone has read this very output.
        let mode = if self.follow {
            WatchMode::Follow
        } else {
            WatchMode::Snapshot
        };
        let depends_on = intent_state
            .stages
            .iter()
            .map(|s| (s.stage_id.clone(), s.depends_on.clone()))
            .collect();
        let mut log_buffers: BTreeMap<String, Vec<DestLog>> = BTreeMap::new();
        let mut announced_parked = false;

        let watch = grpc
            .watch_release(
                intent_id,
                mode,
                ReleaseProgress::with_dependencies(depends_on),
                |event, progress| {
                    use forest_grpc_interface::wait_release_event::Event;
                    if let Some(Event::LogLine(line)) = &event.event {
                        let stderr = matches!(
                            forest_grpc_interface::LogChannel::try_from(line.channel),
                            Ok(forest_grpc_interface::LogChannel::Stderr)
                        );
                        log_buffers
                            .entry(line.destination.clone())
                            .or_default()
                            .push(DestLog {
                                line: line.line.clone(),
                                timestamp: line.timestamp.clone(),
                                stderr,
                            });
                    }
                    if mode == WatchMode::Follow && !announced_parked && progress.is_parked() {
                        announced_parked = true;
                        eprintln!(
                            "awaiting approval: {}; --follow waits until it is approved or rejected",
                            progress.awaiting_approval().join(", ")
                        );
                    }
                },
            )
            .await
            .map_err(|e| {
                tracing::warn!("wait_release stream error: {e:#}");
                e
            })
            .ok();

        // Without the stream, the intent state fetched above still says which
        // stages are parked. Not narrowed by --stage: a parked plan is why the
        // stages after it are pending, so `--stage deploy-prod` needs it too.
        let awaiting: Vec<String> = match &watch {
            Some(w) => w.progress.awaiting_approval(),
            None => intent_state
                .stages
                .iter()
                .filter(|s| s.approval_status.as_deref() == Some("AWAITING_APPROVAL"))
                .map(|s| s.stage_id.clone())
                .collect(),
        };
        let in_progress = watch.as_ref().is_some_and(|w| {
            w.end == WatchEnd::Snapshot && !w.progress.is_terminal() && awaiting.is_empty()
        });

        // With --stage, keep the logs of that stage's destinations only.
        if stages_filter.is_some() {
            log_buffers
                .retain(|dest, _| output.destinations.iter().any(|d| d.destination == *dest));
        }

        output.attach_logs(&log_buffers);
        output.awaiting_approval = awaiting.clone();
        output.in_progress = in_progress;

        let hints: Vec<String> = awaiting
            .iter()
            .map(|stage| super::approval_hint(stage, &output.slug))
            .collect();

        if json_mode {
            let json = serde_json::to_string_pretty(&output).context("serialize show output")?;
            println!("{json}");
        } else if self.logs_only {
            render_logs_only(&output.plan_outputs, &log_buffers);
            // stderr, so `--logs-only > release.log` stays the logs alone.
            for hint in &hints {
                eprintln!("{hint}");
            }
        } else {
            print!("{}", output.render_body(&log_buffers));
            for hint in &hints {
                println!("{hint}");
            }
        }

        if in_progress && !json_mode {
            eprintln!(
                "release is still in progress; follow it with: forest release show {} --follow",
                output.slug
            );
        }

        let any_failed = match &watch {
            Some(w) => w.result.any_failed(),
            None => intent_state
                .steps
                .iter()
                .any(|s| matches!(s.status.as_str(), "FAILED" | "TIMED_OUT" | "CANCELLED")),
        };

        if any_failed {
            anyhow::bail!("release reached a non-success terminal state");
        }

        Ok(())
    }
}

pub(crate) struct ResolvedRelease {
    pub(crate) annotation: ReleaseAnnotation,
    pub(crate) project: Project,
    pub(crate) intent_id: Uuid,
    pub(crate) intent_state: forest_grpc_interface::ReleaseIntentState,
}

/// Treat the target as a release-intent UUID first; fall back to slug.
/// UUIDs are unambiguous so the order avoids accidental collisions.
pub(crate) async fn resolve_target(
    state: &State,
    target: &str,
    organisation_hint: Option<&str>,
) -> anyhow::Result<ResolvedRelease> {
    let grpc = state.grpc_client();

    if let Ok(intent_id) = target.parse::<Uuid>() {
        let organisation = match organisation_hint {
            Some(o) => o.to_string(),
            None => prompt_org_select(state).await?,
        };
        let states = grpc
            .get_release_intent_states(&organisation, None, true)
            .await
            .context("get release intent states")?;
        let intent_state = states
            .release_intents
            .into_iter()
            .find(|i| i.release_intent_id == intent_id.to_string())
            .with_context(|| format!("release-intent {intent_id} not found in {organisation}"))?;
        let artifact_id: Uuid = intent_state.artifact_id.parse().context("artifact_id")?;

        // No GetArtifactById RPC; look up the annotation via project's
        // annotation listing and match on id.
        let annotations = grpc
            .get_release_annotations_by_project(&organisation, &intent_state.project)
            .await
            .context("get release annotations")?;
        let annotation = annotations
            .into_iter()
            .find(|a| a.artifact_id == artifact_id)
            .with_context(|| format!("no annotation found for artifact {artifact_id}"))?;

        let project = Project {
            organisation: organisation.clone(),
            project: intent_state.project.clone(),
        };

        return Ok(ResolvedRelease {
            annotation,
            project,
            intent_id,
            intent_state,
        });
    }

    let (annotation, project) = grpc
        .get_release_annotation_with_project_by_slug(target)
        .await
        .with_context(|| format!("could not resolve release slug '{target}'"))?;

    let states = grpc
        .get_release_intent_states(&project.organisation, Some(&project.project), true)
        .await
        .context("get release intent states")?;

    let intent_state = states
        .release_intents
        .into_iter()
        .find(|i| i.artifact_id == annotation.artifact_id.to_string())
        .with_context(|| format!("no release intent found for slug '{target}'"))?;

    let intent_id: Uuid = intent_state
        .release_intent_id
        .parse()
        .context("release_intent_id")?;

    Ok(ResolvedRelease {
        annotation,
        project,
        intent_id,
        intent_state,
    })
}

pub(crate) async fn pick_release_interactive(
    state: &State,
    organisation_hint: Option<&str>,
    project_hint: Option<&str>,
) -> anyhow::Result<ResolvedRelease> {
    let grpc = state.grpc_client();

    let organisation = match organisation_hint {
        Some(o) => o.to_string(),
        None => prompt_org_select(state).await?,
    };

    let project_name = match project_hint {
        Some(p) => Some(p.to_string()),
        None => prompt_project_select_opt(state, &organisation).await?,
    };

    let (annotations, intent_states) = match project_name.as_deref() {
        Some(p) => {
            let (a, i) = tokio::try_join!(
                grpc.get_release_annotations_by_project(&organisation, p),
                grpc.get_release_intent_states(&organisation, Some(p), true)
            )
            .context("fetch releases")?;
            (a, i)
        }
        None => {
            let i = grpc
                .get_release_intent_states(&organisation, None, true)
                .await
                .context("fetch release intents")?;
            (Vec::new(), i)
        }
    };

    if intent_states.release_intents.is_empty() {
        anyhow::bail!(
            "no releases found for {organisation}{}",
            project_name
                .as_deref()
                .map(|p| format!("/{p}"))
                .unwrap_or_default()
        );
    }

    let annotation_by_artifact: std::collections::HashMap<String, ReleaseAnnotation> = annotations
        .into_iter()
        .map(|a| (a.artifact_id.to_string(), a))
        .collect();

    let mut choices: Vec<IntentPickItem> = intent_states
        .release_intents
        .into_iter()
        .map(|intent| IntentPickItem {
            annotation: annotation_by_artifact.get(&intent.artifact_id).cloned(),
            intent,
        })
        .collect();
    choices.sort_by(|a, b| b.intent.created_at.cmp(&a.intent.created_at));
    choices.truncate(20);

    let picked = inquire::Select::new("Select a release:", choices).prompt()?;

    let intent_id: Uuid = picked
        .intent
        .release_intent_id
        .parse()
        .context("release_intent_id")?;

    let annotation = picked
        .annotation
        .context("release annotation not found for selected intent")?;

    let project = Project {
        organisation,
        project: picked.intent.project.clone(),
    };

    Ok(ResolvedRelease {
        annotation,
        project,
        intent_id,
        intent_state: picked.intent,
    })
}

struct IntentPickItem {
    intent: forest_grpc_interface::ReleaseIntentState,
    annotation: Option<ReleaseAnnotation>,
}

impl Display for IntentPickItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let title = self
            .annotation
            .as_ref()
            .map(|a| a.context.title.as_str())
            .unwrap_or("(no annotation)");
        let slug = self
            .annotation
            .as_ref()
            .map(|a| a.slug.as_str())
            .unwrap_or(self.intent.release_intent_id.as_str());
        let dest_summary = if self.intent.steps.is_empty() {
            "no destinations".to_string()
        } else {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for s in &self.intent.steps {
                *counts.entry(s.status.as_str()).or_default() += 1;
            }
            counts
                .into_iter()
                .map(|(k, v)| format!("{v} {k}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        write!(
            f,
            "{}  {}  [{}]  {}",
            self.intent.created_at, slug, dest_summary, title
        )
    }
}

// ── Output structure (also the JSON shape) ──────────────────────────

#[derive(Serialize)]
struct ShowOutput {
    slug: String,
    release_intent_id: String,
    artifact_id: String,
    project: String,
    created_at: String,
    title: String,
    description: Option<String>,
    actor: ActorRef,
    source: SourceRef,
    stages: Vec<StageView>,
    destinations: Vec<DestView>,
    plan_outputs: Vec<PlanView>,
    logs: Vec<DestLogView>,
    /// Stages parked on a plan approval; `forest release approve` clears them.
    awaiting_approval: Vec<String>,
    /// The snapshot ended before the release did.
    in_progress: bool,
}

#[derive(Serialize)]
struct ActorRef {
    user: Option<String>,
    email: Option<String>,
    source_type: Option<String>,
    run_url: Option<String>,
}

#[derive(Serialize)]
struct SourceRef {
    commit_sha: Option<String>,
    branch: Option<String>,
    repo_url: Option<String>,
    version: Option<String>,
}

#[derive(Serialize)]
struct StageView {
    stage_id: String,
    stage_type: String,
    status: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    error_message: Option<String>,
    approval_status: Option<String>,
}

#[derive(Serialize)]
struct DestView {
    destination: String,
    environment: String,
    status: String,
    error_message: Option<String>,
    stage_id: Option<String>,
    signals: Vec<SignalView>,
}

#[derive(Serialize, Clone)]
struct SignalView {
    name: String,
    status: String,
    detail: String,
    observed_at: String,
    reported_by: String,
}

#[derive(Serialize)]
struct PlanView {
    stage_id: String,
    status: String,
    outputs: Vec<PlanDestView>,
}

#[derive(Serialize)]
struct PlanDestView {
    destination: String,
    status: String,
    plan_output: String,
}

#[derive(Serialize)]
struct DestLogView {
    destination: String,
    lines: Vec<LogLineView>,
}

#[derive(Serialize)]
struct LogLineView {
    line: String,
    timestamp: String,
    channel: &'static str,
}

struct DestLog {
    line: String,
    timestamp: String,
    stderr: bool,
}

impl ShowOutput {
    fn new(
        annotation: &ReleaseAnnotation,
        project: &Project,
        intent_id: &Uuid,
        intent_state: &forest_grpc_interface::ReleaseIntentState,
        plan_outputs: &BTreeMap<String, forest_grpc_interface::GetPlanOutputResponse>,
        stages_filter: Option<&str>,
    ) -> Self {
        let stages: Vec<StageView> = intent_state
            .stages
            .iter()
            .filter(|s| stages_filter.map(|sf| s.stage_id == sf).unwrap_or(true))
            .map(|s| StageView {
                stage_id: s.stage_id.clone(),
                stage_type: stage_type_name(s.stage_type).to_string(),
                status: stage_status_name(s.status).to_string(),
                started_at: s.started_at.clone(),
                completed_at: s.completed_at.clone(),
                error_message: s.error_message.clone(),
                approval_status: s.approval_status.clone(),
            })
            .collect();

        let destinations: Vec<DestView> = intent_state
            .steps
            .iter()
            .filter(|s| {
                stages_filter
                    .map(|sf| s.stage_id.as_deref() == Some(sf))
                    .unwrap_or(true)
            })
            .map(|s| DestView {
                destination: s.destination_name.clone(),
                environment: s.environment.clone(),
                status: s.status.clone(),
                error_message: s.error_message.clone(),
                stage_id: s.stage_id.clone(),
                signals: Vec::new(),
            })
            .collect();

        let plan_views: Vec<PlanView> = plan_outputs
            .iter()
            .map(|(stage_id, resp)| PlanView {
                stage_id: stage_id.clone(),
                status: resp.status.clone(),
                outputs: resp
                    .outputs
                    .iter()
                    .map(|o| PlanDestView {
                        destination: o.destination_name.clone(),
                        status: o.status.clone(),
                        plan_output: o.plan_output.clone(),
                    })
                    .collect(),
            })
            .collect();

        Self {
            slug: annotation.slug.clone(),
            release_intent_id: intent_id.to_string(),
            artifact_id: annotation.artifact_id.to_string(),
            project: format!("{}/{}", project.organisation, project.project),
            created_at: intent_state.created_at.clone(),
            title: annotation.context.title.clone(),
            description: annotation.context.description.clone(),
            actor: ActorRef {
                user: annotation.source.username.clone(),
                email: annotation.source.email.clone(),
                source_type: annotation.source.source_type.clone(),
                run_url: annotation.source.run_url.clone(),
            },
            source: SourceRef {
                commit_sha: annotation.reference.as_ref().map(|r| r.commit_sha.clone()),
                branch: annotation
                    .reference
                    .as_ref()
                    .and_then(|r| r.commit_branch.clone()),
                repo_url: annotation
                    .reference
                    .as_ref()
                    .and_then(|r| r.repo_url.clone()),
                version: annotation
                    .reference
                    .as_ref()
                    .and_then(|r| r.version.clone()),
            },
            stages,
            destinations,
            plan_outputs: plan_views,
            logs: Vec::new(),
            awaiting_approval: Vec::new(),
            in_progress: false,
        }
    }

    fn attach_signals(&mut self, signals: &[forest_grpc_interface::Signal]) {
        for d in &mut self.destinations {
            d.signals = signals
                .iter()
                .filter(|s| s.destination == d.destination && s.environment == d.environment)
                .map(|s| SignalView {
                    name: s.name.clone(),
                    status: signal_status_name(s.status).to_string(),
                    detail: s.detail.clone(),
                    observed_at: s.observed_at.clone(),
                    reported_by: s.reported_by.clone(),
                })
                .collect();
        }
    }

    fn attach_logs(&mut self, buffers: &BTreeMap<String, Vec<DestLog>>) {
        self.logs = buffers
            .iter()
            .map(|(dest, lines)| DestLogView {
                destination: dest.clone(),
                lines: lines
                    .iter()
                    .map(|l| LogLineView {
                        line: l.line.clone(),
                        timestamp: l.timestamp.clone(),
                        channel: if l.stderr { "stderr" } else { "stdout" },
                    })
                    .collect(),
            })
            .collect();
    }

    fn render_header(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "release: {}", self.slug);
        let _ = writeln!(out, "  intent:      {}", self.release_intent_id);
        let _ = writeln!(out, "  project:     {}", self.project);
        let _ = writeln!(out, "  created:     {}", self.created_at);
        let _ = writeln!(out, "  title:       {}", self.title);
        if let Some(desc) = &self.description
            && !desc.is_empty()
        {
            let _ = writeln!(out, "  description: {desc}");
        }
        if let Some(user) = &self.actor.user {
            let email = self
                .actor
                .email
                .as_deref()
                .map(|e| format!(" <{e}>"))
                .unwrap_or_default();
            let _ = writeln!(out, "  actor:       {user}{email}");
        }
        if let Some(sha) = &self.source.commit_sha {
            let branch = self
                .source
                .branch
                .as_deref()
                .map(|b| format!(" ({b})"))
                .unwrap_or_default();
            let _ = writeln!(out, "  source:      {sha}{branch}");
        }
        if let Some(version) = &self.source.version {
            let _ = writeln!(out, "  version:     {version}");
        }
        out.push('\n');
        out
    }

    fn render_body(&self, log_buffers: &BTreeMap<String, Vec<DestLog>>) -> String {
        use std::fmt::Write;
        let mut out = String::new();

        if !self.stages.is_empty() {
            let _ = writeln!(out, "stages:");
            for s in &self.stages {
                render_stage(&mut out, s);
                if let Some(err) = &s.error_message {
                    let _ = writeln!(out, "    error: {err}");
                }
            }
            out.push('\n');
        }

        if !self.destinations.is_empty() {
            let _ = writeln!(out, "destinations:");
            for d in &self.destinations {
                render_destination(&mut out, d);
            }
            out.push('\n');
        }

        let plan_has_content = self
            .plan_outputs
            .iter()
            .any(|p| p.outputs.iter().any(|o| !o.plan_output.is_empty()));
        if plan_has_content {
            let _ = writeln!(out, "plan output:");
            for plan in &self.plan_outputs {
                if !plan.outputs.iter().any(|o| !o.plan_output.is_empty()) {
                    continue;
                }
                let _ = writeln!(out, "  stage {} [{}]:", plan.stage_id, plan.status);
                for o in &plan.outputs {
                    if o.plan_output.is_empty() {
                        continue;
                    }
                    let _ = writeln!(out, "    {} [{}]:", o.destination, o.status);
                    for line in o.plan_output.lines() {
                        let _ = writeln!(out, "      {line}");
                    }
                }
            }
            out.push('\n');
        }

        if !log_buffers.is_empty() {
            let _ = writeln!(out, "logs:");
            for (dest, lines) in log_buffers {
                let _ = writeln!(out, "  {dest}:");
                for line in lines {
                    let _ = writeln!(out, "    {}", line.line);
                }
            }
        }

        out
    }
}

fn render_logs_only(plan_outputs: &[PlanView], log_buffers: &BTreeMap<String, Vec<DestLog>>) {
    for plan in plan_outputs {
        for o in &plan.outputs {
            if o.plan_output.is_empty() {
                continue;
            }
            println!(
                "# plan: {} stage={} ({})",
                o.destination, plan.stage_id, o.status
            );
            println!("{}", o.plan_output);
        }
    }
    for (dest, lines) in log_buffers {
        println!("# deploy: {dest}");
        for line in lines {
            if line.stderr {
                eprintln!("{}", line.line);
            } else {
                println!("{}", line.line);
            }
        }
    }
}

fn render_stage(out: &mut String, s: &StageView) {
    use std::fmt::Write;
    // A parked plan stage stays ACTIVE; without this it reads as running.
    let awaiting = s.approval_status.as_deref() == Some("AWAITING_APPROVAL");
    let _ = writeln!(
        out,
        "  {icon} {sid}: {st}({started}) [{status}{approval}]",
        icon = if awaiting {
            "⏸"
        } else {
            stage_icon(&s.status)
        },
        sid = s.stage_id,
        st = s.stage_type,
        started = s.started_at.as_deref().unwrap_or(""),
        status = s.status,
        approval = if awaiting { ", AWAITING_APPROVAL" } else { "" },
    );
}

fn render_destination(out: &mut String, d: &DestView) {
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "  {icon} [{env}] {dest} [{status}]",
        icon = stage_icon(&d.status),
        env = d.environment,
        dest = d.destination,
        status = d.status,
    );
    if let Some(err) = &d.error_message {
        let _ = writeln!(out, "    error: {err}");
    }
    for s in &d.signals {
        let _ = writeln!(
            out,
            "    {icon} {name} [{status}] {detail}",
            icon = signal_icon(&s.status),
            name = s.name,
            status = s.status,
            detail = s.detail,
        );
    }
}

/// The `HealthStatus` wire value as the name the server stores and a person
/// reads.
fn signal_status_name(status: i32) -> &'static str {
    use forest_grpc_interface::HealthStatus;
    match HealthStatus::try_from(status) {
        Ok(HealthStatus::Healthy) => "HEALTHY",
        Ok(HealthStatus::Progressing) => "PROGRESSING",
        Ok(HealthStatus::Degraded) => "DEGRADED",
        Ok(HealthStatus::Unhealthy) => "UNHEALTHY",
        Ok(HealthStatus::Missing) => "MISSING",
        Ok(HealthStatus::Unspecified) | Err(_) => "UNSPECIFIED",
    }
}

fn signal_icon(status: &str) -> &'static str {
    match status {
        "HEALTHY" => "✓",
        "PROGRESSING" => "▶",
        // A warning, not a cross: the destination succeeded, and this says
        // what that success does not yet mean.
        "DEGRADED" => "⚠",
        "UNHEALTHY" | "MISSING" => "✗",
        _ => "•",
    }
}

fn stage_icon(status: &str) -> &'static str {
    match status {
        "SUCCEEDED" => "✓",
        "ACTIVE" | "RUNNING" | "ASSIGNED" => "▶",
        "FAILED" | "CANCELLED" | "TIMED_OUT" => "✗",
        // Neither a tick nor a cross: it did not run, and it did not go wrong.
        "SUPERSEDED" => "⇥",
        "PENDING" | "QUEUED" => "◌",
        _ => "•",
    }
}

fn is_plan_stage(stage_type: i32) -> bool {
    stage_type == forest_grpc_interface::PipelineRunStageType::Plan as i32
}

fn stage_type_name(stage_type: i32) -> &'static str {
    match forest_grpc_interface::PipelineRunStageType::try_from(stage_type) {
        Ok(forest_grpc_interface::PipelineRunStageType::Deploy) => "deploy",
        Ok(forest_grpc_interface::PipelineRunStageType::Wait) => "wait",
        Ok(forest_grpc_interface::PipelineRunStageType::Plan) => "plan",
        _ => "unknown",
    }
}

fn stage_status_name(status: i32) -> &'static str {
    match forest_grpc_interface::PipelineRunStageStatus::try_from(status) {
        Ok(forest_grpc_interface::PipelineRunStageStatus::Pending) => "PENDING",
        Ok(forest_grpc_interface::PipelineRunStageStatus::Active) => "ACTIVE",
        Ok(forest_grpc_interface::PipelineRunStageStatus::Succeeded) => "SUCCEEDED",
        Ok(forest_grpc_interface::PipelineRunStageStatus::Failed) => "FAILED",
        Ok(forest_grpc_interface::PipelineRunStageStatus::Cancelled) => "CANCELLED",
        Ok(forest_grpc_interface::PipelineRunStageStatus::Superseded) => "SUPERSEDED",
        Ok(forest_grpc_interface::PipelineRunStageStatus::AwaitingApproval) => "AWAITING_APPROVAL",
        _ => "UNSPECIFIED",
    }
}

// ── Small prompt helpers (mirrors `commit.rs`) ──────────────────────

async fn prompt_org_select(state: &State) -> anyhow::Result<String> {
    let resp = state
        .grpc_client()
        .list_my_organisations("")
        .await
        .context("list organisations")?;
    if resp.organisations.is_empty() {
        anyhow::bail!("no organisations available");
    }
    if resp.organisations.len() == 1 {
        return Ok(resp.organisations.into_iter().next().unwrap().name);
    }
    let names: Vec<String> = resp.organisations.into_iter().map(|o| o.name).collect();
    Ok(inquire::Select::new("Organisation:", names).prompt()?)
}

async fn prompt_project_select_opt(
    state: &State,
    organisation: &str,
) -> anyhow::Result<Option<String>> {
    let projects = state
        .grpc_client()
        .get_projects(GetProjectsQuery::Organisation(
            organisation.to_string().into(),
        ))
        .await
        .context("list projects")?;
    if projects.is_empty() {
        return Ok(None);
    }
    if projects.len() == 1 {
        return Ok(Some(projects.into_iter().next().unwrap().to_string()));
    }
    let mut choices: Vec<String> = vec!["(all projects)".to_string()];
    choices.extend(projects.iter().map(|p| p.to_string()));
    let picked = inquire::Select::new("Project:", choices).prompt()?;
    Ok(if picked == "(all projects)" {
        None
    } else {
        Some(picked)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn succeeded_destination(signals: Vec<SignalView>) -> DestView {
        DestView {
            destination: "flux-prod".into(),
            environment: "prod".into(),
            status: "SUCCEEDED".into(),
            error_message: None,
            stage_id: None,
            signals,
        }
    }

    /// The case #298 is about: the destination SUCCEEDED, but Flux was never
    /// told to sync. The warning has to be on the destination's own lines,
    /// not only somewhere in the log dump.
    #[test]
    fn a_degraded_reconcile_shows_under_a_succeeded_destination() {
        let mut out = String::new();
        render_destination(
            &mut out,
            &succeeded_destination(vec![SignalView {
                name: "reconcile".into(),
                status: "DEGRADED".into(),
                detail: "reconcile webhook failed after 4 attempt(s)".into(),
                observed_at: String::new(),
                reported_by: "forest/flux@1".into(),
            }]),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "  ✓ [prod] flux-prod [SUCCEEDED]");
        assert_eq!(
            lines[1],
            "    ⚠ reconcile [DEGRADED] reconcile webhook failed after 4 attempt(s)"
        );
    }

    #[test]
    fn a_destination_without_signals_renders_as_before() {
        let mut out = String::new();
        render_destination(&mut out, &succeeded_destination(Vec::new()));
        assert_eq!(out, "  ✓ [prod] flux-prod [SUCCEEDED]\n");
    }

    fn plan_stage(approval_status: Option<&str>) -> StageView {
        StageView {
            stage_id: "plan-prod".into(),
            stage_type: "plan".into(),
            status: "ACTIVE".into(),
            started_at: Some("2026-09-24T21:12:30Z".into()),
            completed_at: None,
            error_message: None,
            approval_status: approval_status.map(Into::into),
        }
    }

    /// A parked plan stage is ACTIVE on the wire. Shown as bare ACTIVE it
    /// reads as a plan still running, which is how a person ends up waiting
    /// on a release that is waiting on them.
    #[test]
    fn a_parked_plan_stage_says_it_is_awaiting_approval() {
        let mut out = String::new();
        render_stage(&mut out, &plan_stage(Some("AWAITING_APPROVAL")));
        assert_eq!(
            out,
            "  ⏸ plan-prod: plan(2026-09-24T21:12:30Z) [ACTIVE, AWAITING_APPROVAL]\n"
        );
    }

    #[test]
    fn a_running_plan_stage_renders_as_before() {
        let mut out = String::new();
        render_stage(&mut out, &plan_stage(None));
        assert_eq!(out, "  ▶ plan-prod: plan(2026-09-24T21:12:30Z) [ACTIVE]\n");
    }

    #[test]
    fn unknown_wire_statuses_read_as_unspecified_rather_than_healthy() {
        assert_eq!(signal_status_name(0), "UNSPECIFIED");
        assert_eq!(signal_status_name(999), "UNSPECIFIED");
    }
}
