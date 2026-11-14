use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

pub const DM_NOTIFICATION_TYPES: &[&str] =
    &["release_started", "release_succeeded", "release_failed"];

/// Attributions strong enough for org baseline delivery: the authenticated
/// actor ran the release, or the commit author was resolved through a GitHub
/// identity the user linked to Forest themselves. A commit email match is not.
pub const STRONG_ATTRIBUTIONS: &[&str] = &["actor", "github_id"];

pub const DIRECTORY_MATCH_TTL_SECS: i64 = 6 * 3600;

/// Events older than this produce no DM: a replayed backlog must not page people.
pub const MAX_DM_EVENT_AGE_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackBaseline {
    pub integration_id: String,
    /// Whether members without a personal Slack link are reached through the
    /// workspace directory (verified-email match).
    pub directory_mapping: bool,
    /// Org default per DM event type, used when the member has no explicit
    /// personal Slack preference.
    pub dm_defaults: BTreeMap<String, bool>,
    pub updated_by: String,
    pub updated_at: String,
}

impl SlackBaseline {
    /// Settings for an integration whose admin never saved baseline settings:
    /// no directory mapping, and DM defaults follow the channel rules exactly as
    /// DMs did before baseline settings existed.
    pub fn legacy(integration_id: &str, channel_rules: &[super::NotificationRule]) -> Self {
        let dm_defaults = DM_NOTIFICATION_TYPES
            .iter()
            .map(|nt| {
                let on = channel_rules
                    .iter()
                    .find(|r| r.notification_type == *nt)
                    .map(|r| r.enabled)
                    .unwrap_or(true);
                (nt.to_string(), on)
            })
            .collect();
        Self {
            integration_id: integration_id.to_string(),
            directory_mapping: false,
            dm_defaults,
            updated_by: String::new(),
            updated_at: String::new(),
        }
    }

    pub fn org_default(&self, notification_type: &str) -> bool {
        self.dm_defaults
            .get(notification_type)
            .copied()
            .unwrap_or(true)
    }
}

/// A member's own Slack-channel preference for one event type, as stored in
/// Forest. `Inherit` means no row: the org default applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonalPreference {
    On,
    Off,
    Inherit,
}

impl PersonalPreference {
    pub fn from_explicit(explicit: Option<bool>) -> Self {
        match explicit {
            Some(true) => Self::On,
            Some(false) => Self::Off,
            None => Self::Inherit,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Inherit => "inherit",
        }
    }
}

pub fn effective_dm_enabled(personal: PersonalPreference, org_default: bool) -> bool {
    match personal {
        PersonalPreference::On => true,
        PersonalPreference::Off => false,
        PersonalPreference::Inherit => org_default,
    }
}

pub fn notification_type_from_forest(name: &str) -> Option<&'static str> {
    match name {
        "NOTIFICATION_TYPE_RELEASE_ANNOTATED" => Some("release_annotated"),
        "NOTIFICATION_TYPE_RELEASE_STARTED" => Some("release_started"),
        "NOTIFICATION_TYPE_RELEASE_SUCCEEDED" => Some("release_succeeded"),
        "NOTIFICATION_TYPE_RELEASE_FAILED" => Some("release_failed"),
        _ => None,
    }
}

pub fn notification_type_to_forest(name: &str) -> Option<&'static str> {
    match name {
        "release_annotated" => Some("NOTIFICATION_TYPE_RELEASE_ANNOTATED"),
        "release_started" => Some("NOTIFICATION_TYPE_RELEASE_STARTED"),
        "release_succeeded" => Some("NOTIFICATION_TYPE_RELEASE_SUCCEEDED"),
        "release_failed" => Some("NOTIFICATION_TYPE_RELEASE_FAILED"),
        _ => None,
    }
}

pub fn explicit_slack_preferences(
    prefs: &[crate::platform::NotificationPreference],
) -> HashMap<String, bool> {
    prefs
        .iter()
        .filter(|p| p.channel == "NOTIFICATION_CHANNEL_SLACK")
        .filter_map(|p| {
            notification_type_from_forest(&p.notification_type)
                .map(|nt| (nt.to_string(), p.enabled))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackDirectoryUser {
    pub id: String,
    pub team_id: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub is_restricted: bool,
    #[serde(default)]
    pub is_ultra_restricted: bool,
    #[serde(default)]
    pub is_stranger: bool,
    #[serde(default)]
    pub is_email_confirmed: Option<bool>,
    #[serde(default)]
    pub email: Option<String>,
}

/// Slackbot is not flagged `is_bot`, so it is excluded by its fixed id.
const SLACKBOT_ID: &str = "USLACKBOT";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRejection {
    OtherWorkspace,
    Deactivated,
    Bot,
    Guest,
    External,
    EmailUnconfirmed,
    EmailMismatch,
}

impl CandidateRejection {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::OtherWorkspace => "the matching Slack account belongs to another workspace",
            Self::Deactivated => "the matching Slack account is deactivated",
            Self::Bot => "the matching Slack account is a bot",
            Self::Guest => "the matching Slack account is a guest",
            Self::External => "the matching Slack account is external to the workspace",
            Self::EmailUnconfirmed => "the matching Slack account has not confirmed its email",
            Self::EmailMismatch => "Slack returned an account for a different email",
        }
    }
}

pub fn check_candidate(
    user: &SlackDirectoryUser,
    team_id: &str,
    queried_email: &str,
) -> Result<(), CandidateRejection> {
    if user.team_id != team_id {
        return Err(CandidateRejection::OtherWorkspace);
    }
    if user.is_stranger {
        return Err(CandidateRejection::External);
    }
    if user.deleted {
        return Err(CandidateRejection::Deactivated);
    }
    if user.is_bot || user.id == SLACKBOT_ID {
        return Err(CandidateRejection::Bot);
    }
    if user.is_restricted || user.is_ultra_restricted {
        return Err(CandidateRejection::Guest);
    }
    if user.is_email_confirmed == Some(false) {
        return Err(CandidateRejection::EmailUnconfirmed);
    }
    match &user.email {
        Some(email) if email.eq_ignore_ascii_case(queried_email) => Ok(()),
        _ => Err(CandidateRejection::EmailMismatch),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MappingStatus {
    Matched { slack_user_id: String },
    NoVerifiedEmail,
    Unmapped,
    Ambiguous,
    Rejected { reason: CandidateRejection },
}

impl MappingStatus {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Matched { .. } => "matched",
            Self::NoVerifiedEmail => "no_verified_email",
            Self::Unmapped => "unmapped",
            Self::Ambiguous => "ambiguous",
            Self::Rejected { .. } => "rejected",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Matched { .. } => "matched by verified email".into(),
            Self::NoVerifiedEmail => "no verified email in Forest".into(),
            Self::Unmapped => "no Slack account matches a verified email".into(),
            Self::Ambiguous => "verified emails match more than one Slack account".into(),
            Self::Rejected { reason } => reason.describe().into(),
        }
    }
}

/// Combines one lookup per verified email into a single mapping decision.
/// Exactly one acceptable, distinct Slack account is required.
pub fn combine_lookups(
    team_id: &str,
    lookups: &[(String, Option<SlackDirectoryUser>)],
) -> MappingStatus {
    if lookups.is_empty() {
        return MappingStatus::NoVerifiedEmail;
    }
    let mut accepted = BTreeSet::new();
    let mut first_rejection = None;
    for (email, found) in lookups {
        let Some(user) = found else { continue };
        match check_candidate(user, team_id, email) {
            Ok(()) => {
                accepted.insert(user.id.clone());
            }
            Err(reason) => {
                first_rejection.get_or_insert(reason);
            }
        }
    }
    match accepted.len() {
        0 => match first_rejection {
            Some(reason) => MappingStatus::Rejected { reason },
            None => MappingStatus::Unmapped,
        },
        1 => MappingStatus::Matched {
            slack_user_id: accepted.into_iter().next().expect("one element"),
        },
        _ => MappingStatus::Ambiguous,
    }
}

pub fn verified_email_set(emails: &[crate::auth::UserEmail]) -> Vec<String> {
    let set: BTreeSet<String> = emails
        .iter()
        .filter(|e| e.verified)
        .map(|e| e.email.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    set.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryMatch {
    pub integration_id: String,
    pub user_id: String,
    pub emails: Vec<String>,
    pub status: MappingStatus,
    pub checked_at: String,
}

impl DirectoryMatch {
    pub fn is_fresh_for(&self, emails: &[String], now: chrono::DateTime<chrono::Utc>) -> bool {
        if self.emails != emails {
            return false;
        }
        chrono::DateTime::parse_from_rfc3339(&self.checked_at)
            .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds() < DIRECTORY_MATCH_TTL_SECS)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmSkip {
    WeakAttribution,
    NotOrgMember,
    OptedOut,
    OrgDefaultOff,
    Unmapped(MappingStatus),
    ForestUnavailable,
    SlackUnavailable,
    Stale,
}

impl DmSkip {
    pub fn describe(&self) -> String {
        match self {
            Self::WeakAttribution => {
                "release was attributed by commit email only; org baseline needs the actor or a linked GitHub account".into()
            }
            Self::NotOrgMember => "recipient is not a current member of the organisation".into(),
            Self::OptedOut => "recipient turned Slack notifications off for this event".into(),
            Self::OrgDefaultOff => "organisation default is off for this event".into(),
            Self::Unmapped(status) => status.describe(),
            Self::ForestUnavailable => "could not confirm membership or preferences with Forest".into(),
            Self::SlackUnavailable => "Slack directory lookup failed".into(),
            Self::Stale => "event is older than 24 hours".into(),
        }
    }
}

/// Which route carries a DM. Personal links are the member's own choice and
/// win over the org directory mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmRoute {
    Personal { slack_user_id: String },
    Baseline { slack_user_id: String },
}

impl DmRoute {
    pub fn slack_user_id(&self) -> &str {
        match self {
            Self::Personal { slack_user_id } | Self::Baseline { slack_user_id } => slack_user_id,
        }
    }
}

/// The routing decision for one Slack team, before any directory lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamDecision {
    Send(DmRoute),
    NeedsDirectory,
    Skip(DmSkip),
    /// No personal link and the org has not enabled directory mapping: the
    /// pre-baseline behaviour, nothing to report.
    NoRoute,
}

pub fn decide_team(
    personal: PersonalPreference,
    baseline: &SlackBaseline,
    notification_type: &str,
    personal_link: Option<&str>,
    attribution: &str,
) -> TeamDecision {
    if !effective_dm_enabled(personal, baseline.org_default(notification_type)) {
        let skip = if personal == PersonalPreference::Off {
            DmSkip::OptedOut
        } else {
            DmSkip::OrgDefaultOff
        };
        return if personal_link.is_some() || baseline.directory_mapping {
            TeamDecision::Skip(skip)
        } else {
            TeamDecision::NoRoute
        };
    }
    if let Some(slack_user_id) = personal_link {
        return TeamDecision::Send(DmRoute::Personal {
            slack_user_id: slack_user_id.to_string(),
        });
    }
    if !baseline.directory_mapping {
        return TeamDecision::NoRoute;
    }
    if !STRONG_ATTRIBUTIONS.contains(&attribution) {
        return TeamDecision::Skip(DmSkip::WeakAttribution);
    }
    TeamDecision::NeedsDirectory
}

pub fn is_stale(event_timestamp: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    chrono::DateTime::parse_from_rfc3339(event_timestamp)
        .map(|t| (now - t.with_timezone(&chrono::Utc)).num_seconds() > MAX_DM_EVENT_AGE_SECS)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, team: &str, email: &str) -> SlackDirectoryUser {
        SlackDirectoryUser {
            id: id.into(),
            team_id: team.into(),
            deleted: false,
            is_bot: false,
            is_restricted: false,
            is_ultra_restricted: false,
            is_stranger: false,
            is_email_confirmed: Some(true),
            email: Some(email.into()),
        }
    }

    fn baseline(mapping: bool) -> SlackBaseline {
        SlackBaseline {
            integration_id: "i".into(),
            directory_mapping: mapping,
            dm_defaults: DM_NOTIFICATION_TYPES
                .iter()
                .map(|t| (t.to_string(), true))
                .collect(),
            updated_by: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn an_explicit_personal_choice_beats_the_org_default() {
        assert!(!effective_dm_enabled(PersonalPreference::Off, true));
        assert!(effective_dm_enabled(PersonalPreference::On, false));
        assert!(effective_dm_enabled(PersonalPreference::Inherit, true));
        assert!(!effective_dm_enabled(PersonalPreference::Inherit, false));
    }

    #[test]
    fn a_member_who_opted_out_is_not_messaged_even_with_a_personal_link() {
        let d = decide_team(
            PersonalPreference::Off,
            &baseline(true),
            "release_failed",
            Some("U1"),
            "actor",
        );
        assert_eq!(d, TeamDecision::Skip(DmSkip::OptedOut));
    }

    #[test]
    fn a_personal_link_wins_over_the_directory() {
        let d = decide_team(
            PersonalPreference::Inherit,
            &baseline(true),
            "release_failed",
            Some("U1"),
            "email",
        );
        assert_eq!(
            d,
            TeamDecision::Send(DmRoute::Personal {
                slack_user_id: "U1".into()
            })
        );
    }

    #[test]
    fn baseline_delivery_refuses_commit_email_attribution() {
        let d = decide_team(
            PersonalPreference::Inherit,
            &baseline(true),
            "release_failed",
            None,
            "email",
        );
        assert_eq!(d, TeamDecision::Skip(DmSkip::WeakAttribution));
        let d = decide_team(
            PersonalPreference::Inherit,
            &baseline(true),
            "release_failed",
            None,
            "",
        );
        assert_eq!(d, TeamDecision::Skip(DmSkip::WeakAttribution));
    }

    #[test]
    fn baseline_delivery_accepts_the_actor_and_a_linked_github_id() {
        for a in ["actor", "github_id"] {
            let d = decide_team(
                PersonalPreference::Inherit,
                &baseline(true),
                "release_failed",
                None,
                a,
            );
            assert_eq!(d, TeamDecision::NeedsDirectory);
        }
    }

    #[test]
    fn without_mapping_or_link_nothing_is_reported() {
        let d = decide_team(
            PersonalPreference::Inherit,
            &baseline(false),
            "release_failed",
            None,
            "actor",
        );
        assert_eq!(d, TeamDecision::NoRoute);
    }

    #[test]
    fn the_org_default_off_is_a_visible_skip_when_baseline_is_on() {
        let mut b = baseline(true);
        b.dm_defaults.insert("release_started".into(), false);
        let d = decide_team(
            PersonalPreference::Inherit,
            &b,
            "release_started",
            None,
            "actor",
        );
        assert_eq!(d, TeamDecision::Skip(DmSkip::OrgDefaultOff));
        let d = decide_team(PersonalPreference::On, &b, "release_started", None, "actor");
        assert_eq!(d, TeamDecision::NeedsDirectory);
    }

    #[test]
    fn legacy_defaults_follow_the_channel_rules() {
        let rules = vec![super::super::NotificationRule {
            id: "r".into(),
            integration_id: "i".into(),
            notification_type: "release_started".into(),
            enabled: false,
        }];
        let b = SlackBaseline::legacy("i", &rules);
        assert!(!b.org_default("release_started"));
        assert!(b.org_default("release_failed"));
        assert!(!b.directory_mapping);
    }

    #[test]
    fn exactly_one_acceptable_account_is_a_match() {
        let s = combine_lookups("T1", &[("a@x.io".into(), Some(user("U1", "T1", "a@x.io")))]);
        assert_eq!(
            s,
            MappingStatus::Matched {
                slack_user_id: "U1".into()
            }
        );
    }

    #[test]
    fn two_emails_on_the_same_account_still_match() {
        let s = combine_lookups(
            "T1",
            &[
                ("a@x.io".into(), Some(user("U1", "T1", "a@x.io"))),
                ("b@x.io".into(), Some(user("U1", "T1", "b@x.io"))),
            ],
        );
        assert_eq!(
            s,
            MappingStatus::Matched {
                slack_user_id: "U1".into()
            }
        );
    }

    #[test]
    fn emails_on_different_accounts_are_ambiguous() {
        let s = combine_lookups(
            "T1",
            &[
                ("a@x.io".into(), Some(user("U1", "T1", "a@x.io"))),
                ("b@x.io".into(), Some(user("U2", "T1", "b@x.io"))),
            ],
        );
        assert_eq!(s, MappingStatus::Ambiguous);
    }

    #[test]
    fn no_hit_is_unmapped_and_no_email_is_reported_as_such() {
        assert_eq!(
            combine_lookups("T1", &[("a@x.io".into(), None)]),
            MappingStatus::Unmapped
        );
        assert_eq!(combine_lookups("T1", &[]), MappingStatus::NoVerifiedEmail);
    }

    #[test]
    fn unsafe_accounts_are_rejected() {
        let mut u = user("U1", "T1", "a@x.io");
        u.deleted = true;
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::Deactivated)
        );
        let mut u = user("U1", "T1", "a@x.io");
        u.is_bot = true;
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::Bot)
        );
        let u = user("USLACKBOT", "T1", "a@x.io");
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::Bot)
        );
        let mut u = user("U1", "T1", "a@x.io");
        u.is_restricted = true;
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::Guest)
        );
        let mut u = user("U1", "T1", "a@x.io");
        u.is_stranger = true;
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::External)
        );
        let u = user("U1", "T2", "a@x.io");
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::OtherWorkspace)
        );
        let mut u = user("U1", "T1", "a@x.io");
        u.is_email_confirmed = Some(false);
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::EmailUnconfirmed)
        );
        let u = user("U1", "T1", "other@x.io");
        assert_eq!(
            check_candidate(&u, "T1", "a@x.io"),
            Err(CandidateRejection::EmailMismatch)
        );
        let u = user("U1", "T1", "A@X.io");
        assert_eq!(check_candidate(&u, "T1", "a@x.io"), Ok(()));
    }

    #[test]
    fn a_rejected_account_does_not_hide_behind_an_unmapped_email() {
        let mut guest = user("U1", "T1", "a@x.io");
        guest.is_restricted = true;
        let s = combine_lookups(
            "T1",
            &[("a@x.io".into(), Some(guest)), ("b@x.io".into(), None)],
        );
        assert_eq!(
            s,
            MappingStatus::Rejected {
                reason: CandidateRejection::Guest
            }
        );
    }

    #[test]
    fn only_verified_emails_form_the_lookup_set() {
        let emails = vec![
            crate::auth::UserEmail {
                email: "B@x.io".into(),
                verified: true,
            },
            crate::auth::UserEmail {
                email: "unverified@x.io".into(),
                verified: false,
            },
            crate::auth::UserEmail {
                email: "b@x.io".into(),
                verified: true,
            },
            crate::auth::UserEmail {
                email: "a@x.io".into(),
                verified: true,
            },
        ];
        assert_eq!(
            verified_email_set(&emails),
            vec!["a@x.io".to_string(), "b@x.io".to_string()]
        );
    }

    #[test]
    fn a_cached_match_expires_and_is_invalidated_by_an_email_change() {
        let now = chrono::Utc::now();
        let m = DirectoryMatch {
            integration_id: "i".into(),
            user_id: "u".into(),
            emails: vec!["a@x.io".into()],
            status: MappingStatus::Unmapped,
            checked_at: (now - chrono::Duration::minutes(5)).to_rfc3339(),
        };
        assert!(m.is_fresh_for(&["a@x.io".into()], now));
        assert!(!m.is_fresh_for(&["b@x.io".into()], now));
        let old = DirectoryMatch {
            checked_at: (now - chrono::Duration::hours(7)).to_rfc3339(),
            ..m
        };
        assert!(!old.is_fresh_for(&["a@x.io".into()], now));
    }

    #[test]
    fn old_events_are_stale() {
        let now = chrono::Utc::now();
        assert!(is_stale(
            &(now - chrono::Duration::hours(25)).to_rfc3339(),
            now
        ));
        assert!(!is_stale(
            &(now - chrono::Duration::minutes(1)).to_rfc3339(),
            now
        ));
    }

    #[test]
    fn forest_preference_rows_map_to_explicit_slack_choices() {
        let prefs = vec![
            crate::platform::NotificationPreference {
                notification_type: "NOTIFICATION_TYPE_RELEASE_FAILED".into(),
                channel: "NOTIFICATION_CHANNEL_SLACK".into(),
                enabled: false,
            },
            crate::platform::NotificationPreference {
                notification_type: "NOTIFICATION_TYPE_RELEASE_STARTED".into(),
                channel: "NOTIFICATION_CHANNEL_CLI".into(),
                enabled: false,
            },
        ];
        let m = explicit_slack_preferences(&prefs);
        assert_eq!(m.get("release_failed"), Some(&false));
        assert_eq!(m.get("release_started"), None);
    }
}
