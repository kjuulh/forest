//! When to stop reading a `WaitRelease` stream.
//!
//! The server keeps the stream open until the release is terminal. A release
//! whose plan stage is parked awaiting approval is not terminal, and will not
//! become terminal until someone reads the plan and approves it. A client that
//! only follows to the end therefore waits forever for the person who is
//! waiting on it: `forest release show` never printed the plan, and
//! `forest release release --pipeline` never returned.
//!
//! This module decides, from the events alone, when a client has seen enough.
//! It is pure so the rules can be tested without a server.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use forest_grpc_interface::{WaitReleaseEvent, wait_release_event::Event};

/// How long the stream must stay silent before a client takes the server's
/// current pass to be over.
///
/// Only needed against servers that predate the `ReplayComplete` marker. The
/// server sends a pass back to back (tens of milliseconds even for a long plan)
/// and polls again every 2s, and a parked release has nothing new to say on
/// later passes, so a silence this long means the replay is done.
pub const QUIET_GAP: Duration = Duration::from_millis(1500);

/// What the caller wants out of the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchMode {
    /// Everything until the release is terminal (`show --follow`). A parked
    /// release keeps this waiting until someone approves or rejects it.
    Follow,
    /// Follow a release just started, but stop once it parks awaiting approval
    /// (`release release`). Nothing else will happen until a human acts.
    UntilParked,
    /// What has been persisted so far, then stop (`show`).
    Snapshot,
}

/// Why the client stopped reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEnd {
    /// The server closed the stream: the release reached a terminal state.
    Terminal,
    /// Every stage still open is parked on an approval, or blocked behind one.
    Parked,
    /// The replay is complete; the release may still be in flight.
    Snapshot,
}

const TERMINAL_STAGE_STATUSES: [&str; 4] = ["SUCCEEDED", "FAILED", "CANCELLED", "SUPERSEDED"];

#[derive(Clone, Debug, Default)]
struct StageProgress {
    status: String,
    approval_status: Option<String>,
}

impl StageProgress {
    fn is_terminal(&self) -> bool {
        TERMINAL_STAGE_STATUSES.contains(&self.status.as_str())
    }

    /// A parked plan stage stays `ACTIVE`: approval is tracked alongside the
    /// status, not as a value of it.
    fn is_awaiting_approval(&self) -> bool {
        !self.is_terminal() && self.approval_status.as_deref() == Some("AWAITING_APPROVAL")
    }
}

/// What a client has learned about a release from the stream so far.
#[derive(Clone, Debug, Default)]
pub struct ReleaseProgress {
    stages: BTreeMap<String, StageProgress>,
    depends_on: HashMap<String, Vec<String>>,
    /// Whether `depends_on` came from somewhere. Servers before the field
    /// existed send none, and then every stage looks like a root.
    deps_known: bool,
    destinations: BTreeMap<String, String>,
    replay_complete: bool,
}

impl ReleaseProgress {
    /// Start with the stage graph, when the caller has it (from
    /// `GetReleaseIntentStates`). Without it the graph is learned from the
    /// stream, if the server sends it.
    pub fn with_dependencies(depends_on: HashMap<String, Vec<String>>) -> Self {
        Self {
            deps_known: !depends_on.is_empty(),
            depends_on,
            ..Self::default()
        }
    }

    pub fn observe(&mut self, event: &WaitReleaseEvent) {
        match &event.event {
            Some(Event::StageUpdate(stage)) => {
                if !stage.depends_on.is_empty() {
                    self.deps_known = true;
                    self.depends_on
                        .insert(stage.stage_id.clone(), stage.depends_on.clone());
                }
                self.stages.insert(
                    stage.stage_id.clone(),
                    StageProgress {
                        status: stage.status.clone(),
                        approval_status: stage.approval_status.clone(),
                    },
                );
            }
            Some(Event::StatusUpdate(status)) => {
                self.destinations
                    .insert(status.destination.clone(), status.status.clone());
            }
            Some(Event::ReplayComplete(_)) => self.replay_complete = true,
            Some(Event::LogLine(_)) | None => {}
        }
    }

    /// Whether the server has said its replay of persisted history is over.
    pub fn replay_complete(&self) -> bool {
        self.replay_complete
    }

    /// Stages parked on a plan approval, in id order.
    pub fn awaiting_approval(&self) -> Vec<String> {
        self.stages
            .iter()
            .filter(|(_, s)| s.is_awaiting_approval())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether nothing more will happen until a human approves or rejects.
    ///
    /// True when at least one stage is awaiting approval, every other stage is
    /// terminal or PENDING behind a stage that has not succeeded, and no
    /// destination is still queued or running.
    pub fn is_parked(&self) -> bool {
        if self.awaiting_approval().is_empty() {
            return false;
        }

        let stages_idle = self.stages.iter().all(|(id, s)| {
            s.is_terminal()
                || s.is_awaiting_approval()
                || (s.status == "PENDING" && self.is_blocked(id))
        });

        let destinations_idle = self.destinations.values().all(|status| {
            status
                .parse::<forest_models::ReleaseStatus>()
                .is_ok_and(|s| s.is_finalized())
        });

        stages_idle && destinations_idle
    }

    /// Whether everything seen has finished: every stage terminal and every
    /// destination finalized. A snapshot that ended on the replay marker uses
    /// this to tell a finished release from one still in flight.
    pub fn is_terminal(&self) -> bool {
        let seen_anything = !self.stages.is_empty() || !self.destinations.is_empty();
        seen_anything
            && self.stages.values().all(StageProgress::is_terminal)
            && self.destinations.values().all(|status| {
                status
                    .parse::<forest_models::ReleaseStatus>()
                    .is_ok_and(|s| s.is_finalized())
            })
    }

    /// A PENDING stage is blocked when one of its dependencies has not
    /// succeeded. One whose dependencies all succeeded is about to start.
    ///
    /// Without a known graph (an older server, and no intent state from the
    /// caller) a PENDING stage beside a parked one is taken to be blocked by
    /// it: pipelines are chains in practice, and the alternative is following
    /// forever. The worst case is stopping while an unrelated root stage waits
    /// for the coordinator's next sweep, which `show --follow` still covers.
    fn is_blocked(&self, stage_id: &str) -> bool {
        if !self.deps_known {
            return true;
        }
        self.depends_on
            .get(stage_id)
            .map(|deps| {
                deps.iter()
                    .any(|dep| self.stages.get(dep).is_none_or(|d| d.status != "SUCCEEDED"))
            })
            .unwrap_or(false)
    }
}

/// Whether to stop reading right after an event has been observed.
pub fn stop_after_event(mode: WatchMode, progress: &ReleaseProgress) -> Option<WatchEnd> {
    match mode {
        WatchMode::Follow => None,
        WatchMode::Snapshot => progress.replay_complete().then_some(WatchEnd::Snapshot),
        // The marker only follows the first pass, so this catches a release
        // that was already parked when the follow began. A release that parks
        // later is caught by the quiet gap.
        WatchMode::UntilParked => {
            (progress.replay_complete() && progress.is_parked()).then_some(WatchEnd::Parked)
        }
    }
}

/// If the stream stays silent for [`QUIET_GAP`], whether that ends the watch,
/// and as what. `None` means keep waiting however long it takes.
pub fn stop_on_quiet(mode: WatchMode, progress: &ReleaseProgress) -> Option<WatchEnd> {
    match mode {
        WatchMode::Follow => None,
        WatchMode::Snapshot => Some(WatchEnd::Snapshot),
        WatchMode::UntilParked => progress.is_parked().then_some(WatchEnd::Parked),
    }
}

#[cfg(test)]
mod tests {
    use forest_grpc_interface::{
        PipelineStageUpdate, ReleaseLogLine, ReleaseStatusUpdate, ReplayComplete,
    };

    use super::*;

    fn stage(id: &str, status: &str, approval: Option<&str>, deps: &[&str]) -> WaitReleaseEvent {
        WaitReleaseEvent {
            event: Some(Event::StageUpdate(PipelineStageUpdate {
                stage_id: id.into(),
                stage_type: "plan".into(),
                status: status.into(),
                approval_status: approval.map(Into::into),
                depends_on: deps.iter().map(|d| d.to_string()).collect(),
                ..Default::default()
            })),
        }
    }

    fn destination(name: &str, status: &str) -> WaitReleaseEvent {
        WaitReleaseEvent {
            event: Some(Event::StatusUpdate(ReleaseStatusUpdate {
                destination: name.into(),
                status: status.into(),
            })),
        }
    }

    fn log(line: &str) -> WaitReleaseEvent {
        WaitReleaseEvent {
            event: Some(Event::LogLine(ReleaseLogLine {
                destination: "grund-infrastructure-prod".into(),
                line: line.into(),
                ..Default::default()
            })),
        }
    }

    fn replay_complete() -> WaitReleaseEvent {
        WaitReleaseEvent {
            event: Some(Event::ReplayComplete(ReplayComplete {})),
        }
    }

    fn progress(events: &[WaitReleaseEvent]) -> ReleaseProgress {
        let mut p = ReleaseProgress::default();
        for e in events {
            p.observe(e);
        }
        p
    }

    /// The live case that hung `show`: plan-prod ran its plan and parked,
    /// deploy-prod waits on it. Exactly what the server streamed for
    /// `unhappily-trim-brill` on 2026-09-24, minus the log lines.
    fn parked_plan_prod() -> Vec<WaitReleaseEvent> {
        vec![
            stage("deploy-prod", "PENDING", None, &["plan-prod"]),
            stage("plan-prod", "ACTIVE", Some("AWAITING_APPROVAL"), &[]),
            destination("grund-infrastructure-prod", "SUCCEEDED"),
            log("Plan: 2 to add, 2 to change, 0 to destroy."),
        ]
    }

    #[test]
    fn a_plan_stage_awaiting_approval_with_its_dependent_pending_is_parked() {
        let p = progress(&parked_plan_prod());
        assert!(p.is_parked());
        assert_eq!(p.awaiting_approval(), vec!["plan-prod".to_string()]);
    }

    #[test]
    fn a_plan_stage_still_running_its_plan_is_not_parked() {
        let p = progress(&[
            stage("deploy-prod", "PENDING", None, &["plan-prod"]),
            stage("plan-prod", "ACTIVE", None, &[]),
            destination("grund-infrastructure-prod", "RUNNING"),
        ]);
        assert!(!p.is_parked());
        assert!(p.awaiting_approval().is_empty());
    }

    /// The stage flips to awaiting approval only once its release rows are
    /// done, but a status update can lag the stage update within a pass.
    #[test]
    fn a_destination_still_running_keeps_the_release_from_being_parked() {
        let p = progress(&[
            stage("plan-prod", "ACTIVE", Some("AWAITING_APPROVAL"), &[]),
            destination("grund-infrastructure-prod", "RUNNING"),
        ]);
        assert!(!p.is_parked());
    }

    #[test]
    fn a_pending_stage_whose_dependencies_succeeded_is_about_to_run_so_not_parked() {
        let p = progress(&[
            stage("plan-dev", "SUCCEEDED", Some("APPROVED"), &[]),
            stage("deploy-dev", "PENDING", None, &["plan-dev"]),
            stage("plan-prod", "ACTIVE", Some("AWAITING_APPROVAL"), &[]),
        ]);
        assert!(!p.is_parked());
    }

    #[test]
    fn an_independent_stage_still_deploying_keeps_the_release_from_being_parked() {
        let p = progress(&[
            stage("deploy-dev", "ACTIVE", None, &[]),
            stage("plan-prod", "ACTIVE", Some("AWAITING_APPROVAL"), &[]),
        ]);
        assert!(!p.is_parked());
    }

    #[test]
    fn an_approved_stage_is_no_longer_awaiting_approval() {
        let p = progress(&[
            stage("plan-prod", "SUCCEEDED", Some("APPROVED"), &[]),
            stage("deploy-prod", "ACTIVE", None, &["plan-prod"]),
        ]);
        assert!(p.awaiting_approval().is_empty());
        assert!(!p.is_parked());
    }

    /// Servers before `depends_on` was streamed send none. The intent state
    /// the caller already fetched supplies the graph instead.
    #[test]
    fn the_graph_from_the_intent_state_is_used_when_the_stream_has_none() {
        let mut p = ReleaseProgress::with_dependencies(HashMap::from([
            ("deploy-prod".to_string(), vec!["plan-prod".to_string()]),
            ("plan-prod".to_string(), vec![]),
            ("deploy-dev".to_string(), vec![]),
        ]));
        for e in [
            stage("deploy-dev", "PENDING", None, &[]),
            stage("deploy-prod", "PENDING", None, &[]),
            stage("plan-prod", "ACTIVE", Some("AWAITING_APPROVAL"), &[]),
        ] {
            p.observe(&e);
        }
        // deploy-dev is a root about to start; not parked yet.
        assert!(!p.is_parked());
    }

    #[test]
    fn without_any_graph_a_pending_stage_beside_a_parked_one_counts_as_blocked() {
        let p = progress(&[
            stage("deploy-prod", "PENDING", None, &[]),
            stage("plan-prod", "ACTIVE", Some("AWAITING_APPROVAL"), &[]),
            destination("grund-infrastructure-prod", "SUCCEEDED"),
        ]);
        assert!(p.is_parked());
    }

    #[test]
    fn a_parked_release_is_not_terminal_and_a_finished_one_is() {
        assert!(!progress(&parked_plan_prod()).is_terminal());
        assert!(
            progress(&[
                stage("plan-prod", "SUCCEEDED", Some("APPROVED"), &[]),
                stage("deploy-prod", "SUCCEEDED", None, &["plan-prod"]),
                destination("grund-infrastructure-prod", "SUCCEEDED"),
            ])
            .is_terminal()
        );
        assert!(!ReleaseProgress::default().is_terminal());
    }

    #[test]
    fn follow_never_stops_early_even_when_parked() {
        let mut events = parked_plan_prod();
        events.push(replay_complete());
        let p = progress(&events);
        assert_eq!(stop_after_event(WatchMode::Follow, &p), None);
        assert_eq!(stop_on_quiet(WatchMode::Follow, &p), None);
    }

    #[test]
    fn a_snapshot_stops_at_the_replay_marker() {
        let p = progress(&parked_plan_prod());
        assert_eq!(stop_after_event(WatchMode::Snapshot, &p), None);

        let mut events = parked_plan_prod();
        events.push(replay_complete());
        let p = progress(&events);
        assert_eq!(
            stop_after_event(WatchMode::Snapshot, &p),
            Some(WatchEnd::Snapshot)
        );
    }

    /// Against a server without the marker, a snapshot ends on silence,
    /// whether or not the release is parked: in flight, it ends after the
    /// first pass, which is the replay.
    #[test]
    fn a_snapshot_stops_on_silence_parked_or_not() {
        assert_eq!(
            stop_on_quiet(WatchMode::Snapshot, &progress(&parked_plan_prod())),
            Some(WatchEnd::Snapshot)
        );
        assert_eq!(
            stop_on_quiet(WatchMode::Snapshot, &ReleaseProgress::default()),
            Some(WatchEnd::Snapshot)
        );
    }

    #[test]
    fn until_parked_stops_on_silence_only_once_parked() {
        let running = progress(&[
            stage("plan-prod", "ACTIVE", None, &[]),
            destination("grund-infrastructure-prod", "RUNNING"),
        ]);
        assert_eq!(stop_on_quiet(WatchMode::UntilParked, &running), None);
        assert_eq!(
            stop_on_quiet(WatchMode::UntilParked, &progress(&parked_plan_prod())),
            Some(WatchEnd::Parked)
        );
    }

    #[test]
    fn until_parked_stops_at_the_marker_when_already_parked() {
        let mut events = parked_plan_prod();
        events.push(replay_complete());
        assert_eq!(
            stop_after_event(WatchMode::UntilParked, &progress(&events)),
            Some(WatchEnd::Parked)
        );
        // The marker alone does not end a follow of a release still running.
        let running = progress(&[stage("plan-prod", "ACTIVE", None, &[]), replay_complete()]);
        assert_eq!(stop_after_event(WatchMode::UntilParked, &running), None);
    }
}
