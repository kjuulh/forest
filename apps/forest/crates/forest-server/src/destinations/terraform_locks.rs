use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use forest_models::Destination;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::replicas::Liveness;

pub const DEFAULT_STALE_LOCK_AFTER: Duration = Duration::from_secs(15 * 60);
pub const LEGACY_LOCK_MIN_AGE: Duration = Duration::from_secs(60 * 60);

const IN_PROCESS_PREFIX: &str = "release:";
const RUNNER_PREFIX: &str = "runner:";

const TERMINAL_STATUSES: &[&str] = &[
    "SUCCEEDED",
    "FAILED",
    "CANCELLED",
    "TIMED_OUT",
    "SUPERSEDED",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    #[serde(rename = "ID", alias = "lock_id", default)]
    pub id: String,
    #[serde(rename = "Operation", default)]
    pub operation: String,
    #[serde(rename = "Info", default)]
    pub info: String,
    #[serde(rename = "Who", default)]
    pub who: String,
    #[serde(rename = "Version", default)]
    pub version: String,
    #[serde(rename = "Created", default)]
    pub created: String,
    #[serde(rename = "Path", default)]
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    InProcessRelease {
        release_id: Uuid,
        replica: Option<String>,
    },
    RunnerRelease {
        release_id: Option<Uuid>,
        runner: String,
    },
    Operator {
        principal: String,
    },
    Legacy,
}

impl Holder {
    pub fn parse(principal: Option<&str>) -> Self {
        let Some(principal) = principal else {
            return Holder::Legacy;
        };
        if let Some(rest) = principal.strip_prefix(IN_PROCESS_PREFIX) {
            let id_part = rest.split_once(':').map_or(rest, |(id, _)| id);
            let (id, replica) = match id_part.split_once('@') {
                Some((id, replica)) => (id, Some(replica.to_string())),
                None => (id_part, None),
            };
            if let Ok(release_id) = Uuid::parse_str(id) {
                return Holder::InProcessRelease {
                    release_id,
                    replica: replica.filter(|r| !r.is_empty()),
                };
            }
        }
        if let Some(rest) = principal.strip_prefix(RUNNER_PREFIX) {
            return match rest
                .split_once(':')
                .and_then(|(id, runner)| Uuid::parse_str(id).ok().map(|id| (id, runner)))
            {
                Some((release_id, runner)) => Holder::RunnerRelease {
                    release_id: Some(release_id),
                    runner: runner.to_string(),
                },
                None => Holder::RunnerRelease {
                    release_id: None,
                    runner: rest.to_string(),
                },
            };
        }
        Holder::Operator {
            principal: principal.to_string(),
        }
    }

    pub fn release_id(&self) -> Option<Uuid> {
        match self {
            Holder::InProcessRelease { release_id, .. } => Some(*release_id),
            Holder::RunnerRelease { release_id, .. } => *release_id,
            _ => None,
        }
    }

    pub fn replica(&self) -> Option<&str> {
        match self {
            Holder::InProcessRelease { replica, .. } => replica.as_deref(),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Holder::InProcessRelease {
                release_id,
                replica: Some(replica),
            } => format!("forest release {release_id} on {replica}"),
            Holder::InProcessRelease {
                release_id,
                replica: None,
            } => format!("forest release {release_id}"),
            Holder::RunnerRelease {
                release_id: Some(release_id),
                runner,
            } => format!("forest runner {runner} for release {release_id}"),
            Holder::RunnerRelease {
                release_id: None,
                runner,
            } => format!("forest runner {runner}"),
            Holder::Operator { principal } => principal.clone(),
            Holder::Legacy => "an unknown holder".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateLock {
    pub state_id: String,
    pub lock_id: String,
    pub operation: String,
    pub who: String,
    pub info: String,
    pub tf_version: String,
    pub tf_created: String,
    pub path: String,
    pub principal: Option<String>,
    pub release_id: Option<Uuid>,
    pub replica: Option<String>,
    pub acquired_at: DateTime<Utc>,
}

impl StateLock {
    pub fn holder_kind(&self) -> Holder {
        match Holder::parse(self.principal.as_deref()) {
            Holder::InProcessRelease {
                release_id,
                replica: None,
            } => Holder::InProcessRelease {
                release_id,
                replica: self.replica.clone(),
            },
            holder => holder,
        }
    }

    pub fn holder(&self) -> String {
        self.holder_kind().describe()
    }

    pub fn conflict_info(&self) -> LockInfo {
        let who = match self.who.as_str() {
            "" => self.holder(),
            tf_who => format!("{} ({tf_who})", self.holder()),
        };
        let info = match self.info.as_str() {
            "" => format!("held by {}", self.holder()),
            tf_info => format!("held by {}; {tf_info}", self.holder()),
        };
        let created = match DateTime::parse_from_rfc3339(&self.tf_created) {
            Ok(_) => self.tf_created.clone(),
            Err(_) => self
                .acquired_at
                .to_rfc3339_opts(SecondsFormat::Micros, true),
        };
        LockInfo {
            id: self.lock_id.clone(),
            operation: self.operation.clone(),
            info,
            who,
            version: self.tf_version.clone(),
            created,
            path: self.path.clone(),
        }
    }

    pub fn age(&self, now: DateTime<Utc>) -> Duration {
        (now - self.acquired_at).to_std().unwrap_or_default()
    }

    pub fn since(&self) -> String {
        self.acquired_at.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    pub fn describe(&self) -> String {
        let operation = match self.operation.as_str() {
            "" => "unknown operation".to_string(),
            op => op.to_string(),
        };
        format!(
            "lock {} ({operation}) held by {} since {}",
            self.lock_id,
            self.holder(),
            self.since()
        )
    }
}

pub fn in_process_principal(release_id: Uuid, replica: &str, destination: &Destination) -> String {
    format!(
        "{IN_PROCESS_PREFIX}{release_id}@{replica}:{}/{}",
        destination.organisation, destination.name
    )
}

pub fn runner_principal(release_id: Uuid, runner_id: &str) -> String {
    format!("{RUNNER_PREFIX}{release_id}:{runner_id}")
}

pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    InProcessRelease {
        release_id: Uuid,
        status: Option<String>,
        replica: Option<String>,
        replica_liveness: Liveness,
    },
    RunnerRelease,
    Operator,
    Legacy {
        other_release_active: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleLockDecision {
    Release(String),
    Keep(String),
}

fn minutes(d: Duration) -> String {
    format!("{}m", d.as_secs() / 60)
}

pub fn decide(
    lock: &StateLock,
    owner: &OwnerState,
    now: DateTime<Utc>,
    stale_after: Duration,
) -> StaleLockDecision {
    let age = minutes(lock.age(now));
    let unlock_hint = "clear it with `forest state unlock` once you know it is not in use";

    match owner {
        OwnerState::InProcessRelease {
            release_id,
            status,
            replica,
            replica_liveness,
        } => {
            let on = replica
                .as_deref()
                .map(|r| format!(" on replica {r}"))
                .unwrap_or_default();
            match status.as_deref() {
                None => StaleLockDecision::Keep(format!(
                    "locked by release {release_id}{on}, which forest has no record of; {unlock_hint}"
                )),
                Some(status) if !is_terminal(status) => StaleLockDecision::Keep(format!(
                    "locked by release {release_id}{on}, still {}",
                    status.to_lowercase()
                )),
                Some(_) if replica.is_none() => StaleLockDecision::Keep(format!(
                    "locked by release {release_id}, whose replica is not recorded; {unlock_hint}"
                )),
                Some(status) => match replica_liveness {
                    Liveness::Alive => StaleLockDecision::Keep(format!(
                        "locked by release {release_id}{on}, still alive"
                    )),
                    Liveness::Silent { last_seen } => StaleLockDecision::Keep(format!(
                        "locked by release {release_id}{on}, not heard from since {}; a silent \
                         replica may still be applying, so the lock is kept until it has been \
                         silent for FOREST_REPLICA_GONE_SECS",
                        last_seen.to_rfc3339_opts(SecondsFormat::Secs, true)
                    )),
                    Liveness::Stopped => StaleLockDecision::Release(format!(
                        "release {release_id} that held it is {} and its replica {} shut down \
                         cleanly",
                        status.to_lowercase(),
                        replica.as_deref().unwrap_or_default()
                    )),
                    Liveness::Gone => StaleLockDecision::Release(format!(
                        "release {release_id} that held it is {} and its replica {} has been \
                         silent for longer than FOREST_REPLICA_GONE_SECS",
                        status.to_lowercase(),
                        replica.as_deref().unwrap_or_default()
                    )),
                },
            }
        }
        OwnerState::RunnerRelease => StaleLockDecision::Keep(format!(
            "locked by {} since {} ({age} ago); a remote runner's lock is never cleared \
             automatically, {unlock_hint}",
            lock.holder(),
            lock.since()
        )),
        OwnerState::Operator => StaleLockDecision::Keep(format!(
            "locked by {} since {}; clear it with `forest state unlock`",
            lock.holder(),
            lock.since()
        )),
        OwnerState::Legacy { .. } if lock.age(now) < stale_after.max(LEGACY_LOCK_MIN_AGE) => {
            StaleLockDecision::Keep(format!(
                "{} has no recorded owner and is only {age} old (stale after {})",
                lock.describe(),
                minutes(stale_after.max(LEGACY_LOCK_MIN_AGE))
            ))
        }
        OwnerState::Legacy {
            other_release_active: true,
        } => StaleLockDecision::Keep(format!(
            "{} has no recorded owner and another release for this destination is assigned or \
             running",
            lock.describe()
        )),
        OwnerState::Legacy {
            other_release_active: false,
        } => StaleLockDecision::Release(format!(
            "it has no recorded owner, no other release for this destination is assigned or \
             running and it is {age} old"
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleLockCheck {
    Unlocked,
    HeldByThisRelease,
    Kept { lock: StateLock, reason: String },
    Released { lock: StateLock, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE: Duration = DEFAULT_STALE_LOCK_AFTER;

    fn lock(age_minutes: i64, principal: Option<String>) -> (StateLock, DateTime<Utc>) {
        let now = Utc::now();
        let holder = Holder::parse(principal.as_deref());
        (
            StateLock {
                state_id: "dev.project".into(),
                lock_id: "c7081117".into(),
                operation: "OperationTypeApply".into(),
                who: "root@ip-10-0-0-1".into(),
                info: String::new(),
                tf_version: "1.12.6".into(),
                tf_created: String::new(),
                path: String::new(),
                release_id: holder.release_id(),
                replica: holder.replica().map(str::to_string),
                principal,
                acquired_at: now - chrono::Duration::minutes(age_minutes),
            },
            now,
        )
    }

    fn in_process(
        id: Uuid,
        status: Option<&str>,
        replica: Option<&str>,
        replica_liveness: Liveness,
    ) -> OwnerState {
        OwnerState::InProcessRelease {
            release_id: id,
            status: status.map(str::to_string),
            replica: replica.map(str::to_string),
            replica_liveness,
        }
    }

    fn destination() -> Destination {
        Destination {
            organisation: "understory".into(),
            name: "dev/eu-west-1/ecs".into(),
            environment: "dev".into(),
            metadata: Default::default(),
            sensitive_keys: vec![],
            destination_type: forest_models::DestinationType {
                organisation: "forest".into(),
                name: "terraform".into(),
                version: 1,
                description: String::new(),
                fields: vec![],
            },
        }
    }

    #[test]
    fn a_lock_whose_release_is_still_running_is_kept_however_old() {
        let id = Uuid::new_v4();
        let (lock, now) = lock(600, None);
        assert_eq!(
            decide(
                &lock,
                &in_process(id, Some("RUNNING"), Some("r1"), Liveness::Gone),
                now,
                STALE
            ),
            StaleLockDecision::Keep(format!(
                "locked by release {id} on replica r1, still running"
            ))
        );
    }

    #[test]
    fn a_finished_releases_lock_is_kept_while_its_replica_is_alive() {
        let id = Uuid::new_v4();
        let (lock, now) = lock(600, None);
        for status in ["FAILED", "TIMED_OUT", "CANCELLED", "SUCCEEDED"] {
            assert_eq!(
                decide(
                    &lock,
                    &in_process(id, Some(status), Some("r1"), Liveness::Alive),
                    now,
                    STALE
                ),
                StaleLockDecision::Keep(format!(
                    "locked by release {id} on replica r1, still alive"
                )),
                "{status}"
            );
        }
    }

    #[test]
    fn a_finished_releases_lock_is_released_once_its_replica_stopped_or_is_long_gone() {
        let id = Uuid::new_v4();
        let (lock, now) = lock(1, None);
        for status in ["FAILED", "TIMED_OUT", "CANCELLED", "SUCCEEDED"] {
            for liveness in [Liveness::Stopped, Liveness::Gone] {
                let decision = decide(
                    &lock,
                    &in_process(id, Some(status), Some("r1"), liveness.clone()),
                    now,
                    STALE,
                );
                assert!(
                    matches!(decision, StaleLockDecision::Release(_)),
                    "{status} {liveness:?}: {decision:?}"
                );
            }
        }
    }

    #[test]
    fn a_finished_releases_lock_is_kept_while_its_replica_is_only_silent() {
        let id = Uuid::new_v4();
        let (lock, now) = lock(600, None);
        let decision = decide(
            &lock,
            &in_process(
                id,
                Some("FAILED"),
                Some("r1"),
                Liveness::Silent {
                    last_seen: now - chrono::Duration::minutes(10),
                },
            ),
            now,
            STALE,
        );
        match decision {
            StaleLockDecision::Keep(reason) => assert!(
                reason.starts_with(&format!(
                    "locked by release {id} on replica r1, not heard from since "
                )),
                "{reason}"
            ),
            other => panic!("a silent replica may be stalled mid-apply: {other:?}"),
        }
    }

    #[test]
    fn a_running_releases_lock_is_kept_even_if_its_replica_stopped() {
        let id = Uuid::new_v4();
        let (lock, now) = lock(600, None);
        assert!(matches!(
            decide(
                &lock,
                &in_process(id, Some("RUNNING"), Some("r1"), Liveness::Stopped),
                now,
                STALE
            ),
            StaleLockDecision::Keep(_)
        ));
    }

    #[test]
    fn a_release_lock_with_no_release_or_replica_on_record_is_kept() {
        let id = Uuid::new_v4();
        let (lock, now) = lock(600, None);
        assert!(matches!(
            decide(
                &lock,
                &in_process(id, None, Some("r1"), Liveness::Gone),
                now,
                STALE
            ),
            StaleLockDecision::Keep(_)
        ));
        assert!(matches!(
            decide(
                &lock,
                &in_process(id, Some("FAILED"), None, Liveness::Gone),
                now,
                STALE
            ),
            StaleLockDecision::Keep(_)
        ));
    }

    #[test]
    fn runner_and_human_locks_are_never_released_however_old() {
        let release = Uuid::new_v4();
        let (runner, now) = lock(6000, Some(runner_principal(release, "runner-7")));
        match decide(&runner, &OwnerState::RunnerRelease, now, STALE) {
            StaleLockDecision::Keep(reason) => assert!(
                reason.contains("forest runner runner-7") && reason.contains("6000m ago"),
                "{reason}"
            ),
            other => panic!("{other:?}"),
        }

        let (human, now) = lock(6000, Some("user:3f2a".into()));
        match decide(&human, &OwnerState::Operator, now, STALE) {
            StaleLockDecision::Keep(reason) => assert!(
                reason.starts_with("locked by user:3f2a since ")
                    && reason.ends_with("clear it with `forest state unlock`"),
                "{reason}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_legacy_lock_is_released_only_after_an_hour_and_when_the_destination_is_idle() {
        let idle = OwnerState::Legacy {
            other_release_active: false,
        };
        let busy = OwnerState::Legacy {
            other_release_active: true,
        };

        let (old, now) = lock(61, None);
        assert!(matches!(
            decide(&old, &idle, now, STALE),
            StaleLockDecision::Release(_)
        ));
        assert!(matches!(
            decide(&old, &busy, now, STALE),
            StaleLockDecision::Keep(_)
        ));

        let (under_an_hour, now) = lock(59, None);
        for stale_after in [STALE, Duration::from_secs(60)] {
            assert!(
                matches!(
                    decide(&under_an_hour, &idle, now, stale_after),
                    StaleLockDecision::Keep(_)
                ),
                "a lock with no recorded holder may be a person's or an old replica's long run"
            );
        }

        let (old, now) = lock(100, None);
        assert!(matches!(
            decide(&old, &idle, now, Duration::from_secs(120 * 60)),
            StaleLockDecision::Keep(_)
        ));
    }

    #[test]
    fn principals_say_what_kind_of_holder_took_the_lock() {
        let id = Uuid::new_v4();
        let principal = in_process_principal(id, "ip-10-0-1-23#ab12cd34", &destination());
        assert_eq!(
            principal,
            format!("release:{id}@ip-10-0-1-23#ab12cd34:understory/dev/eu-west-1/ecs")
        );
        assert_eq!(
            Holder::parse(Some(&principal)),
            Holder::InProcessRelease {
                release_id: id,
                replica: Some("ip-10-0-1-23#ab12cd34".into())
            }
        );
        assert_eq!(
            Holder::parse(Some(&runner_principal(id, "runner:7"))),
            Holder::RunnerRelease {
                release_id: Some(id),
                runner: "runner:7".into()
            }
        );
        assert_eq!(
            Holder::parse(Some("runner:understory/dev")),
            Holder::RunnerRelease {
                release_id: None,
                runner: "understory/dev".into()
            }
        );
        for human in [
            "user:abc",
            "app:x",
            "service_account:y",
            "release:not-a-uuid:x/y",
        ] {
            assert_eq!(
                Holder::parse(Some(human)),
                Holder::Operator {
                    principal: human.into()
                }
            );
        }
        assert_eq!(Holder::parse(None), Holder::Legacy);
    }

    #[test]
    fn the_conflict_terraform_shows_says_which_release_holds_the_lock() {
        let id = Uuid::new_v4();
        let (lock, _) = lock(
            3,
            Some(in_process_principal(
                id,
                "forest-server-abc",
                &destination(),
            )),
        );
        let info = lock.conflict_info();
        assert_eq!(info.id, "c7081117");
        assert_eq!(info.operation, "OperationTypeApply");
        assert_eq!(
            info.who,
            format!("forest release {id} on forest-server-abc (root@ip-10-0-0-1)")
        );
        assert!(info.info.contains(&id.to_string()), "{}", info.info);
        assert!(
            DateTime::parse_from_rfc3339(&info.created).is_ok(),
            "terraform unmarshals Created as a time, so it is always RFC 3339: {}",
            info.created
        );
    }

    #[test]
    fn a_lock_from_before_owners_were_recorded_still_reports_a_holder() {
        let (mut lock, _) = lock(3, None);
        lock.who = String::new();
        let info = lock.conflict_info();
        assert_eq!(info.who, "an unknown holder");
        assert!(DateTime::parse_from_rfc3339(&info.created).is_ok());
    }

    #[test]
    fn terraforms_lock_body_is_read_in_full() {
        let body = r#"{"ID":"c7081117-0000","Operation":"OperationTypePlan","Info":"","Who":"root@host","Version":"1.12.6","Created":"2026-11-19T10:00:00.123456789Z","Path":""}"#;
        let info: LockInfo = serde_json::from_str(body).unwrap();
        assert_eq!(info.id, "c7081117-0000");
        assert_eq!(info.operation, "OperationTypePlan");
        assert_eq!(info.who, "root@host");
        assert_eq!(info.version, "1.12.6");
        assert_eq!(info.created, "2026-11-19T10:00:00.123456789Z");

        let minimal: LockInfo = serde_json::from_str(r#"{"ID":"l1"}"#).unwrap();
        assert_eq!(minimal.id, "l1");
        assert_eq!(minimal.operation, "");
    }
}
