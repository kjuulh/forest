use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::DestinationStatus;
use super::router::{NotificationEvent, SlackMessage};
use crate::platform::{Artifact, PipelineRunStageState, PlanOutput, ReleaseStepState};

/// Emoji names without colons. An empty name falls back to the standard emoji,
/// so messages render before any custom emoji is uploaded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackEmojiSet {
    pub queued: String,
    pub running: String,
    pub succeeded: String,
    pub failed: String,
    pub approval: String,
    pub skipped: String,
    pub rolled_back: String,
    pub celebrate: String,
    pub mourn: String,
}

/// (field, standard fallback, label) for every configurable emoji.
pub const EMOJI_SLOTS: &[(&str, &str, &str)] = &[
    ("queued", "hourglass_flowing_sand", "Queued stage"),
    (
        "running",
        "arrows_counterclockwise",
        "Running stage and in-progress header",
    ),
    ("succeeded", "white_check_mark", "Finished stage"),
    ("failed", "x", "Failed stage"),
    ("approval", "raised_hand", "Waiting for approval"),
    (
        "skipped",
        "black_right_pointing_double_triangle_with_vertical_bar",
        "Skipped or cancelled stage",
    ),
    ("rolled_back", "leftwards_arrow_with_hook", "Rolled back"),
    ("celebrate", "tada", "Header when the release succeeded"),
    ("mourn", "rotating_light", "Header when the release failed"),
];

impl SlackEmojiSet {
    fn slot(&self, field: &str) -> &str {
        match field {
            "queued" => &self.queued,
            "running" => &self.running,
            "succeeded" => &self.succeeded,
            "failed" => &self.failed,
            "approval" => &self.approval,
            "skipped" => &self.skipped,
            "rolled_back" => &self.rolled_back,
            "celebrate" => &self.celebrate,
            "mourn" => &self.mourn,
            _ => "",
        }
    }

    pub fn get(&self, field: &str) -> String {
        let configured = self.slot(field);
        if !configured.is_empty() {
            return format!(":{configured}:");
        }
        let fallback = EMOJI_SLOTS
            .iter()
            .find(|(f, _, _)| *f == field)
            .map(|(_, d, _)| *d)
            .unwrap_or("grey_question");
        format!(":{fallback}:")
    }

    /// Rejects names Slack could never have, so a typo can't break every message.
    pub fn from_fields(fields: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut set = Self::default();
        for (field, _, _) in EMOJI_SLOTS {
            let raw = fields.get(*field).map(|s| s.trim()).unwrap_or("");
            let name = raw.trim_matches(':').to_string();
            if !name.is_empty() && !is_valid_emoji_name(&name) {
                return Err(format!("\"{raw}\" is not a valid Slack emoji name"));
            }
            let slot = match *field {
                "queued" => &mut set.queued,
                "running" => &mut set.running,
                "succeeded" => &mut set.succeeded,
                "failed" => &mut set.failed,
                "approval" => &mut set.approval,
                "skipped" => &mut set.skipped,
                "rolled_back" => &mut set.rolled_back,
                "celebrate" => &mut set.celebrate,
                "mourn" => &mut set.mourn,
                _ => continue,
            };
            *slot = name;
        }
        Ok(set)
    }

    pub fn configured(&self, field: &str) -> &str {
        self.slot(field)
    }
}

fn is_valid_emoji_name(name: &str) -> bool {
    name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-+'".contains(c))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Queued,
    Running,
    Succeeded,
    Failed,
    AwaitingApproval,
    Skipped,
    RolledBack,
}

impl StageState {
    fn from_status(status: &str) -> Self {
        match status.to_ascii_uppercase().as_str() {
            "RUNNING" | "ACTIVE" | "STARTED" | "ASSIGNED" | "AWAITING_SIGNAL" => Self::Running,
            "SUCCEEDED" => Self::Succeeded,
            "FAILED" | "TIMED_OUT" => Self::Failed,
            "AWAITING_APPROVAL" => Self::AwaitingApproval,
            "CANCELLED" | "SUPERSEDED" | "SKIPPED" => Self::Skipped,
            "ROLLED_BACK" => Self::RolledBack,
            _ => Self::Queued,
        }
    }

    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::RolledBack
        )
    }

    fn emoji(self, set: &SlackEmojiSet) -> String {
        set.get(match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::AwaitingApproval => "approval",
            Self::Skipped => "skipped",
            Self::RolledBack => "rolled_back",
        })
    }
}

/// The release as a whole. `Succeeded` only once every stage (or, without a
/// pipeline, every destination) has finished successfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverallState {
    InProgress,
    WaitingForApproval,
    Succeeded,
    Failed,
    Cancelled,
}

pub struct ReleaseView<'a> {
    pub event: &'a NotificationEvent,
    pub destinations: &'a HashMap<String, DestinationStatus>,
    pub stages: &'a [PipelineRunStageState],
    pub steps: &'a [ReleaseStepState],
    pub forage_url: &'a str,
    pub emoji: &'a SlackEmojiSet,
    /// The most destinations any event for this release announced. Forest
    /// sets the count on start events only, so the caller keeps the maximum.
    pub expected_destinations: usize,
}

struct Row {
    state: StageState,
    label: String,
    started: Option<DateTime<Utc>>,
    completed: Option<DateTime<Utc>>,
}

fn parse_ts(s: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s?)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// A stage's state with the events folded in: Forest may emit a destination's
/// terminal event a moment before it marks the stage, and the message must not
/// sit on "running" when no further event will come.
fn stage_state(view: &ReleaseView, stage: &PipelineRunStageState) -> StageState {
    let state = StageState::from_status(&stage.status);
    if state.is_terminal() || stage.stage_type != "deploy" {
        return state;
    }
    let names: Vec<&str> = view
        .steps
        .iter()
        .filter(|s| s.stage_id.as_deref() == Some(stage.stage_id.as_str()))
        .map(|s| s.destination_name.as_str())
        .collect();
    if names.is_empty() {
        return state;
    }
    let seen: Vec<&DestinationStatus> = names
        .iter()
        .filter_map(|n| view.destinations.get(*n))
        .collect();
    if seen.iter().any(|d| d.status == "failed") {
        StageState::Failed
    } else if seen.len() == names.len() && seen.iter().all(|d| d.status == "succeeded") {
        StageState::Succeeded
    } else {
        state
    }
}

fn rows(view: &ReleaseView) -> Vec<Row> {
    if !view.stages.is_empty() {
        return super::router::topo_sort_stages(view.stages)
            .iter()
            .map(|s| {
                let state = stage_state(view, s);
                Row {
                    state,
                    label: stage_label(s, state),
                    started: parse_ts(s.started_at.as_deref().or(s.queued_at.as_deref())),
                    completed: parse_ts(s.completed_at.as_deref()),
                }
            })
            .collect();
    }

    let mut names: Vec<&str> = view
        .destinations
        .keys()
        .map(String::as_str)
        .chain(view.steps.iter().map(|s| s.destination_name.as_str()))
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut out: Vec<Row> = names
        .into_iter()
        .map(|name| {
            let step = view.steps.iter().find(|s| s.destination_name == name);
            let event = view.destinations.get(name);
            let state = match (event, step) {
                (Some(d), _) => StageState::from_status(&d.status),
                (None, Some(s)) => StageState::from_status(&s.status),
                (None, None) => StageState::Queued,
            };
            let verb = match state {
                StageState::Succeeded => "Deployed",
                StageState::Running => "Deploying",
                StageState::Failed => "Failed to deploy",
                StageState::Skipped => "Skipped",
                _ => "Deploy",
            };
            let (started, completed) = destination_times(event, step);
            Row {
                state,
                label: format!("{verb} `{}`", esc(name)),
                started,
                completed,
            }
        })
        .collect();
    for _ in out.len()..view.expected_destinations {
        out.push(Row {
            state: StageState::Queued,
            label: "Deploy (not started yet)".into(),
            started: None,
            completed: None,
        });
    }
    out
}

/// Forest's step timestamps win: the event carries only one timestamp, and a
/// release started for several destinations names none of them.
fn destination_times(
    event: Option<&DestinationStatus>,
    step: Option<&ReleaseStepState>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let started = step
        .and_then(|s| parse_ts(s.started_at.as_deref().or(s.assigned_at.as_deref())))
        .or_else(|| event.and_then(|d| parse_ts(d.started_at.as_deref())));
    let completed = step
        .and_then(|s| parse_ts(s.completed_at.as_deref()))
        .or_else(|| event.and_then(|d| parse_ts(d.completed_at.as_deref())));
    match (started, completed) {
        (Some(a), Some(b)) if a >= b => (None, Some(b)),
        other => other,
    }
}

/// Slack treats `&`, `<` and `>` as control characters in mrkdwn: an
/// unescaped commit title could ping `<!channel>` or forge a link.
pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn link(url: &str, label: &str) -> String {
    if url.starts_with("https://") && !url.contains(['|', '<', '>', ' ']) {
        format!("<{url}|{label}>")
    } else {
        label.to_string()
    }
}

fn stage_label(stage: &PipelineRunStageState, state: StageState) -> String {
    let env = esc(stage.environment.as_deref().unwrap_or("unknown"));
    match stage.stage_type.as_str() {
        "deploy" => match state {
            StageState::Succeeded => format!("Deployed to `{env}`"),
            StageState::Running => format!("Deploying to `{env}`"),
            StageState::Failed => format!("Deploy to `{env}` failed"),
            StageState::Skipped => format!("Deploy to `{env}` skipped"),
            StageState::RolledBack => format!("Deploy to `{env}` rolled back"),
            _ => format!("Deploy to `{env}`"),
        },
        "plan" => match state {
            StageState::Succeeded => format!("Plan for `{env}` approved"),
            StageState::Running => format!("Planning `{env}`"),
            StageState::AwaitingApproval => format!("Plan for `{env}` needs approval"),
            StageState::Failed => format!("Plan for `{env}` failed"),
            _ => format!("Plan `{env}`"),
        },
        "wait" => {
            let d = format_duration(stage.duration_seconds.unwrap_or(0));
            match state {
                StageState::Succeeded => format!("Waited {d}"),
                StageState::Running => match parse_ts(stage.wait_until.as_deref()) {
                    Some(until) => format!("Waiting {d}, until {}", slack_time(until)),
                    None => format!("Waiting {d}"),
                },
                _ => format!("Wait {d}"),
            }
        }
        "gate" => {
            if stage.gate_waiting_on.is_empty() || state.is_terminal() {
                "Gate".to_string()
            } else {
                format!("Gate, waiting on {}", esc(&stage.gate_waiting_on.join(", ")))
            }
        }
        _ => format!("Stage `{}`", esc(&stage.stage_id)),
    }
}

pub fn overall_state(view: &ReleaseView) -> OverallState {
    if view.destinations.values().any(|d| d.status == "failed") {
        return OverallState::Failed;
    }
    let states: Vec<StageState> = rows(view).iter().map(|r| r.state).collect();
    if states.is_empty() {
        return match view.event.notification_type.as_str() {
            "release_failed" => OverallState::Failed,
            _ => OverallState::InProgress,
        };
    }
    if states.contains(&StageState::Failed) || states.contains(&StageState::RolledBack) {
        OverallState::Failed
    } else if states.contains(&StageState::AwaitingApproval) {
        OverallState::WaitingForApproval
    } else if !states.iter().all(|s| s.is_terminal()) {
        OverallState::InProgress
    } else if states.contains(&StageState::Succeeded) {
        OverallState::Succeeded
    } else {
        OverallState::Cancelled
    }
}

pub fn format_duration(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 if secs % 60 == 0 => format!("{}m", secs / 60),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

fn slack_time(t: DateTime<Utc>) -> String {
    format!(
        "<!date^{}^{{time}}|{} UTC>",
        t.timestamp(),
        t.format("%H:%M")
    )
}

pub fn release_url(view: &ReleaseView) -> String {
    let Some(r) = view.event.release.as_ref() else {
        return String::new();
    };
    if view.forage_url.is_empty() {
        return String::new();
    }
    format!(
        "{}/orgs/{}/projects/{}/releases/{}",
        view.forage_url.trim_end_matches('/'),
        view.event.organisation,
        view.event.project,
        r.slug,
    )
}

pub fn render_parent(view: &ReleaseView) -> SlackMessage {
    let event = view.event;
    let target = esc(&format!("{}/{}", event.organisation, event.project));
    let overall = overall_state(view);
    let e = view.emoji;
    let (emoji, headline, color) = match overall {
        OverallState::InProgress => (e.get("running"), format!("Releasing {target}"), "#0d6efd"),
        OverallState::WaitingForApproval => (
            e.get("approval"),
            format!("Waiting for approval: {target}"),
            "#f0ad4e",
        ),
        OverallState::Succeeded => (e.get("celebrate"), format!("Released {target}"), "#36a64f"),
        OverallState::Failed => (
            e.get("mourn"),
            format!("Release failed: {target}"),
            "#dc3545",
        ),
        OverallState::Cancelled => (
            e.get("skipped"),
            format!("Release cancelled: {target}"),
            "#6c757d",
        ),
    };

    let mut blocks: Vec<Value> = Vec::new();
    let mut header = json!({
        "type": "section",
        "text": { "type": "mrkdwn", "text": format!("{emoji} *{headline}*") }
    });
    let url = release_url(view);
    if !url.is_empty() {
        header["accessory"] = json!({
            "type": "button",
            "text": { "type": "plain_text", "text": "View release" },
            "url": url,
        });
    }
    blocks.push(header);

    if let Some(r) = event.release.as_ref() {
        if !r.context_title.is_empty() {
            blocks.push(json!({
                "type": "section",
                "text": { "type": "mrkdwn", "text": format!(":memo: {}", truncate(&esc(&r.context_title), 500)) }
            }));
        }
        blocks.push(metadata_context(event));
    }

    let rows = rows(view);
    if !rows.is_empty() {
        let lines: Vec<String> = rows
            .iter()
            .map(|row| {
                let mut line = format!("{}  {}", row.state.emoji(e), row.label);
                match (row.state, row.started, row.completed) {
                    (s, Some(a), Some(b)) if s.is_terminal() => {
                        line.push_str(&format!("  ·  {}", format_duration((b - a).num_seconds())));
                    }
                    (StageState::Running, Some(a), _) => {
                        line.push_str(&format!("  ·  since {}", slack_time(a)));
                    }
                    _ => {}
                }
                line
            })
            .collect();
        blocks.push(json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": lines.join("\n") }
        }));

        let done = rows.iter().filter(|r| r.state.is_terminal()).count();
        let first_start = rows.iter().filter_map(|r| r.started).min();
        let last_end = rows.iter().filter_map(|r| r.completed).max();
        let noun = if view.stages.is_empty() {
            "destinations"
        } else {
            "stages"
        };
        let mut footer = match overall {
            OverallState::Succeeded | OverallState::Failed | OverallState::Cancelled => {
                format!("{done}/{} {noun} finished", rows.len())
            }
            _ => format!("{done}/{} {noun} complete", rows.len()),
        };
        let finished = matches!(
            overall,
            OverallState::Succeeded | OverallState::Failed | OverallState::Cancelled
        );
        match (finished, first_start, last_end) {
            (true, Some(a), Some(b)) => footer.push_str(&format!(
                "  ·  took {}",
                format_duration((b - a).num_seconds())
            )),
            (false, Some(a), _) => footer.push_str(&format!("  ·  started {}", slack_time(a))),
            _ => {}
        }
        if overall == OverallState::Failed {
            footer.push_str("  ·  details in thread");
        }
        let skipped = rows.iter().filter(|r| r.state == StageState::Skipped).count();
        if skipped > 0 && overall == OverallState::Succeeded {
            footer.push_str(&format!("  ·  {skipped} skipped"));
        }
        blocks.push(json!({
            "type": "context",
            "elements": [{ "type": "mrkdwn", "text": footer }]
        }));
    }

    SlackMessage {
        text: headline,
        color: color.to_string(),
        blocks,
    }
}

fn metadata_context(event: &NotificationEvent) -> Value {
    let r = event.release.as_ref().expect("caller checked release");
    let mut parts = vec![format!("*{}* / {}", esc(&event.project), esc(&r.slug))];
    if !r.commit_branch.is_empty() {
        parts.push(format!("`{}`", esc(&r.commit_branch)));
    }
    if !r.commit_sha.is_empty() {
        parts.push(format!("`{}`", esc(short_sha(&r.commit_sha))));
    }
    if !r.source_username.is_empty() {
        parts.push(esc(&r.source_username));
    }
    if !r.context_web.is_empty() {
        parts.push(link(&r.context_web, "source"));
    }
    json!({
        "type": "context",
        "elements": [{ "type": "mrkdwn", "text": parts.join("  ·  ") }]
    })
}

fn short_sha(sha: &str) -> &str {
    sha.char_indices().nth(7).map_or(sha, |(i, _)| &sha[..i])
}

/// One thread reply. `key` makes it idempotent: a key is posted at most once
/// per parent message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadPost {
    pub key: String,
    pub text: String,
    pub blocks: Vec<Value>,
}

impl ThreadPost {
    fn new(key: String, text: String) -> Self {
        let blocks = vec![json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": truncate(&text, 2900) }
        })];
        Self {
            key,
            text: truncate(&text, 3000),
            blocks,
        }
    }

    pub fn for_direct_message(&self) -> bool {
        self.key.starts_with("approval:") || self.key.starts_with("failure:")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(1);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[derive(Default)]
pub struct ThreadDetail<'a> {
    pub artifacts: Option<&'a [Artifact]>,
    pub plans: HashMap<String, &'a PlanOutput>,
}

/// Every reply the current state calls for, including ones already posted:
/// the caller skips keys it has posted.
pub fn thread_posts(view: &ReleaseView, detail: &ThreadDetail) -> Vec<ThreadPost> {
    let mut posts = Vec::new();
    let url = release_url(view);

    if let Some(artifacts) = detail.artifacts
        && let Some(post) = changes_post(view, artifacts)
    {
        posts.push(post);
    }

    for stage in super::router::topo_sort_stages(view.stages) {
        let state = stage_state(view, &stage);
        let env = esc(stage.environment.as_deref().unwrap_or("unknown"));
        if stage.stage_type == "plan" {
            if let Some(plan) = detail.plans.get(&stage.stage_id) {
                posts.push(plan_post(&stage.stage_id, &env, plan));
            }
            if state == StageState::AwaitingApproval {
                let link = if url.is_empty() {
                    "in Forest".to_string()
                } else {
                    format!("in <{url}|Forest>")
                };
                posts.push(ThreadPost::new(
                    format!("approval:{}:requested", stage.stage_id),
                    format!(
                        "{} *Approval needed to deploy to `{env}`.* Review the plan and approve or reject it {link}. Any member of *{}* can approve.",
                        view.emoji.get("approval"),
                        esc(&view.event.organisation),
                    ),
                ));
            }
            let decision = stage
                .approval_status
                .as_deref()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if stage.auto_approve != Some(true)
                && (decision == "approved" || decision == "rejected")
            {
                let emoji = if decision == "approved" {
                    view.emoji.get("succeeded")
                } else {
                    view.emoji.get("failed")
                };
                posts.push(ThreadPost::new(
                    format!("approval:{}:{decision}", stage.stage_id),
                    format!("{emoji} Plan for `{env}` was {decision}."),
                ));
            }
        }
        let reported_by_destination = view
            .steps
            .iter()
            .filter(|s| s.stage_id.as_deref() == Some(stage.stage_id.as_str()))
            .any(|s| {
                view.destinations
                    .get(&s.destination_name)
                    .is_some_and(|d| d.status == "failed")
            });
        if state == StageState::Failed
            && !reported_by_destination
            && let Some(err) = stage.error_message.as_deref().filter(|e| !e.is_empty())
        {
            posts.push(failure_post(
                format!("failure:stage:{}", stage.stage_id),
                &format!("{} `{env}`", stage_noun(&stage.stage_type)),
                err,
                &url,
                view.emoji,
            ));
        }
    }

    let mut dests: Vec<_> = view.destinations.iter().collect();
    dests.sort_by_key(|(name, _)| name.as_str());
    for (name, d) in dests {
        match d.status.as_str() {
            "succeeded" => {
                let step = view.steps.iter().find(|s| s.destination_name == *name);
                let took = match destination_times(Some(d), step) {
                    (Some(a), Some(b)) => format!(" in {}", format_duration((b - a).num_seconds())),
                    _ => String::new(),
                };
                let logs = if url.is_empty() {
                    String::new()
                } else {
                    format!("  ·  <{url}|logs>")
                };
                posts.push(ThreadPost::new(
                    format!("deploy:{name}"),
                    format!(
                        "{} Deployed `{}` ({}){took}{logs}",
                        view.emoji.get("succeeded"),
                        esc(name),
                        esc(&d.environment)
                    ),
                ));
            }
            "failed" => posts.push(failure_post(
                format!("failure:{name}"),
                &format!("Deploy to `{}` ({})", esc(name), esc(&d.environment)),
                d.error
                    .as_deref()
                    .unwrap_or("no error message was reported"),
                &url,
                view.emoji,
            )),
            _ => {}
        }
    }

    posts
}

fn stage_noun(stage_type: &str) -> &'static str {
    match stage_type {
        "plan" => "Plan for",
        "deploy" => "Deploy to",
        _ => "Stage for",
    }
}

fn failure_post(
    key: String,
    what: &str,
    err: &str,
    url: &str,
    emoji: &SlackEmojiSet,
) -> ThreadPost {
    let excerpt = esc(&truncate(err.trim(), 1500).replace("```", "'''"));
    let logs = if url.is_empty() {
        String::new()
    } else {
        format!("\nFull logs: <{url}|release in Forest>")
    };
    ThreadPost::new(
        key,
        format!(
            "{} *{what} failed*\n```{excerpt}```{logs}",
            emoji.get("failed")
        ),
    )
}

/// Parses terraform's `Plan: 1 to add, 2 to change, 0 to destroy.` line.
/// `Some((0, 0, 0))` for "No changes.", `None` when there is no summary.
pub fn plan_summary(output: &str) -> Option<(u64, u64, u64)> {
    if output.contains("No changes.") {
        return Some((0, 0, 0));
    }
    let line = output.lines().rev().find(|l| l.contains("Plan:"))?;
    let count = |what: &str| -> u64 {
        line.split(',')
            .find(|part| part.contains(what))
            .and_then(|part| part.split_whitespace().rev().find_map(|w| w.parse().ok()))
            .unwrap_or(0)
    };
    Some((count("to add"), count("to change"), count("to destroy")))
}

fn plan_post(stage_id: &str, env: &str, plan: &PlanOutput) -> ThreadPost {
    let mut outputs: Vec<(String, String)> = plan
        .outputs
        .iter()
        .map(|o| (esc(&o.destination_name), o.plan_output.clone()))
        .collect();
    if outputs.is_empty() && !plan.plan_output.is_empty() {
        outputs.push((env.to_string(), plan.plan_output.clone()));
    }
    let mut lines = vec![format!(":clipboard: *Plan for `{env}`*")];
    for (name, out) in outputs {
        let line = match plan_summary(&out) {
            Some((0, 0, 0)) => format!("• `{name}`: no changes"),
            Some((add, change, destroy)) => {
                let mut l = format!("• `{name}`: +{add} ~{change} -{destroy}");
                if destroy > 0 {
                    l.push_str(&format!(
                        "  :rotating_light: *destroys {destroy} resource{}*",
                        if destroy == 1 { "" } else { "s" }
                    ));
                }
                l
            }
            None => format!("• `{name}`: no plan summary found"),
        };
        lines.push(line);
    }
    ThreadPost::new(format!("plan:{stage_id}"), lines.join("\n"))
}

fn changes_post(view: &ReleaseView, artifacts: &[Artifact]) -> Option<ThreadPost> {
    const SHOWN: usize = 15;
    let current_id = view.event.release.as_ref()?.artifact_id.as_str();
    let mut sorted: Vec<&Artifact> = artifacts.iter().collect();
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let start = sorted.iter().position(|a| a.artifact_id == current_id)?;
    let ours: HashSet<&str> = sorted[start]
        .destinations
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let deployed_before = |a: &Artifact| {
        a.destinations.iter().any(|d| {
            (ours.is_empty() || ours.contains(d.name.as_str()))
                && d.status
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("succeeded"))
        })
    };
    let previous = sorted[start + 1..].iter().position(|a| deployed_before(a));
    let end = previous.map(|p| start + 1 + p).unwrap_or(sorted.len());
    let changes = &sorted[start..end];
    if changes.len() < 2 {
        return None;
    }

    let mut lines = vec![match previous {
        Some(p) => format!(
            ":scroll: *{} changes since the previous release* (`{}`)",
            changes.len(),
            esc(&sorted[start + 1 + p].slug)
        ),
        None => format!(":scroll: *{} changes in this first release*", changes.len()),
    }];
    for a in changes.iter().take(SHOWN) {
        let sha = a
            .git_ref
            .as_ref()
            .map(|r| short_sha(&r.commit_sha))
            .unwrap_or("");
        let sha = match a.context.web.as_deref() {
            Some(web) if !sha.is_empty() => link(web, &format!("`{}`", esc(sha))),
            _ if !sha.is_empty() => format!("`{}`", esc(sha)),
            _ => String::new(),
        };
        let mut line = format!("• {sha} {}", truncate(&esc(a.context.title.trim()), 200)).replace("•  ", "• ");
        if let Some(user) = a.source.as_ref().and_then(|s| s.user.as_deref()) {
            line.push_str(&format!(" — {}", esc(user)));
        }
        if let Some(pr) = a.context.pr.as_deref().filter(|p| !p.is_empty()) {
            line.push_str(&format!(" ({})", link(pr, "PR")));
        }
        lines.push(line);
    }
    if changes.len() > SHOWN {
        lines.push(format!("…and {} more", changes.len() - SHOWN));
    }
    Some(ThreadPost::new("changes".into(), lines.join("\n")))
}

#[cfg(test)]
#[path = "slack_release_tests.rs"]
mod tests;
