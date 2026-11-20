use std::collections::{HashMap, HashSet};

use super::baseline::{
    DM_NOTIFICATION_TYPES, DirectoryMatch, DmRoute, DmSkip, MappingStatus, PersonalPreference,
    SlackBaseline, SlackDirectoryUser, TeamDecision, combine_lookups, decide_team, is_stale,
    verified_email_set,
};
use super::router::{DispatchTask, NotificationEvent, format_slack_message};
use super::{DeliveryStatus, Integration, IntegrationConfig, IntegrationStore};

#[derive(Debug, Clone)]
pub struct RecipientProfile {
    pub user_id: String,
    pub username: String,
    pub emails: Vec<crate::auth::UserEmail>,
    /// Numeric GitHub ids the user linked to Forest themselves.
    pub github_user_ids: Vec<i64>,
}

#[async_trait::async_trait]
pub trait RecipientDirectory: Send + Sync {
    /// `Ok(None)` when the user does not exist or is not a current member of
    /// `organisation`. An `Err` must never be read as membership.
    async fn member(
        &self,
        organisation: &str,
        user_id: &str,
    ) -> Result<Option<RecipientProfile>, String>;

    async fn slack_preferences(&self, user_id: &str) -> Result<HashMap<String, bool>, String>;
}

#[async_trait::async_trait]
pub trait SlackDirectory: Send + Sync {
    /// `Ok(None)` when Slack has no active account for the email.
    async fn lookup_by_email(
        &self,
        access_token: &str,
        email: &str,
    ) -> Result<Option<SlackDirectoryUser>, String>;
}

#[derive(Debug, Clone)]
pub struct DmSkipRecord {
    pub integration_id: String,
    pub user_id: String,
    pub username: String,
    pub reason: DmSkip,
}

#[derive(Debug, Default)]
pub struct DmPlan {
    pub tasks: Vec<DispatchTask>,
    pub skips: Vec<DmSkipRecord>,
}

pub struct SlackTeam {
    pub integration: Integration,
    pub team_id: String,
    pub access_token: String,
    pub baseline: SlackBaseline,
}

/// One sending integration per Slack team, oldest first: two integrations on
/// the same workspace must not DM the same person twice.
pub async fn slack_teams(store: &dyn IntegrationStore, organisation: &str) -> Vec<SlackTeam> {
    let mut integrations = match store.list_integrations(organisation).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(org = %organisation, error = %e, "failed to list integrations for DM routing");
            return vec![];
        }
    };
    integrations.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    let mut seen = HashSet::new();
    let mut teams = Vec::new();
    for integration in integrations.into_iter().filter(|i| i.enabled) {
        let IntegrationConfig::Slack {
            team_id,
            access_token,
            ..
        } = &integration.config
        else {
            continue;
        };
        if team_id.is_empty() || access_token.is_empty() || !seen.insert(team_id.clone()) {
            continue;
        }
        let baseline = load_baseline(store, &integration.id).await;
        teams.push(SlackTeam {
            team_id: team_id.clone(),
            access_token: access_token.clone(),
            baseline,
            integration,
        });
    }
    teams
}

/// The integration whose baseline governs DMs for `team_id`, as chosen by
/// [`slack_teams`]. Settings saved on any other integration would never apply.
pub async fn sending_integration(
    store: &dyn IntegrationStore,
    organisation: &str,
    team_id: &str,
) -> Option<Integration> {
    slack_teams(store, organisation)
        .await
        .into_iter()
        .find(|t| t.team_id == team_id)
        .map(|t| t.integration)
}

pub async fn load_baseline(store: &dyn IntegrationStore, integration_id: &str) -> SlackBaseline {
    match store.get_slack_baseline(integration_id).await {
        Ok(Some(b)) => b,
        _ => {
            let rules = store.list_rules(integration_id).await.unwrap_or_default();
            SlackBaseline::legacy(integration_id, &rules)
        }
    }
}

/// Decides the Slack account of `profile` in one workspace from its verified
/// Forest emails, reusing a fresh cached decision. Provider errors are returned,
/// never cached and never turned into a recipient.
pub async fn resolve_directory_match(
    store: &dyn IntegrationStore,
    slack: &dyn SlackDirectory,
    team: &SlackTeam,
    profile: &RecipientProfile,
    now: chrono::DateTime<chrono::Utc>,
    force: bool,
) -> Result<MappingStatus, String> {
    let emails = verified_email_set(&profile.emails);
    if emails.is_empty() {
        return Ok(MappingStatus::NoVerifiedEmail);
    }
    if !force
        && let Ok(Some(cached)) = store
            .get_directory_match(&team.integration.id, &profile.user_id)
            .await
        && cached.is_fresh_for(&emails, now)
    {
        return Ok(cached.status);
    }

    let mut lookups = Vec::with_capacity(emails.len());
    for email in &emails {
        let found = slack.lookup_by_email(&team.access_token, email).await?;
        lookups.push((email.clone(), found));
    }
    let status = combine_lookups(&team.team_id, &lookups);

    let record = DirectoryMatch {
        integration_id: team.integration.id.clone(),
        user_id: profile.user_id.clone(),
        emails,
        status: status.clone(),
        checked_at: now.to_rfc3339(),
    };
    if let Err(e) = store.upsert_directory_match(&record).await {
        tracing::warn!(error = %e, "failed to cache slack directory match");
    }
    Ok(status)
}

pub async fn plan_direct_messages(
    store: &dyn IntegrationStore,
    recipients: Option<&dyn RecipientDirectory>,
    slack: Option<&dyn SlackDirectory>,
    event: &NotificationEvent,
    now: chrono::DateTime<chrono::Utc>,
) -> DmPlan {
    let mut plan = DmPlan::default();
    if !DM_NOTIFICATION_TYPES.contains(&event.notification_type.as_str()) {
        return plan;
    }
    let Some(release) = event
        .release
        .as_ref()
        .filter(|r| !r.source_user_id.is_empty())
    else {
        return plan;
    };
    let user_id = release.source_user_id.as_str();

    let mut candidates = Vec::new();
    for team in slack_teams(store, &event.organisation).await {
        let link = match store.get_slack_user_link(user_id, &team.team_id).await {
            Ok(link) => link.map(|l| l.slack_user_id),
            Err(e) => {
                tracing::warn!(team_id = %team.team_id, error = %e, "failed to read slack user link");
                None
            }
        };
        if link.is_some() || team.baseline.directory_mapping {
            candidates.push((team, link));
        }
    }
    if candidates.is_empty() {
        return plan;
    }

    let skip_all =
        |plan: &mut DmPlan, candidates: &[(SlackTeam, Option<String>)], reason: DmSkip| {
            for (team, _) in candidates {
                plan.skips.push(DmSkipRecord {
                    integration_id: team.integration.id.clone(),
                    user_id: user_id.to_string(),
                    username: release.source_username.clone(),
                    reason: reason.clone(),
                });
            }
        };

    if is_stale(&event.timestamp, now) {
        skip_all(&mut plan, &candidates, DmSkip::Stale);
        return plan;
    }
    let Some(recipients) = recipients else {
        skip_all(&mut plan, &candidates, DmSkip::ForestUnavailable);
        return plan;
    };
    let profile = match recipients.member(&event.organisation, user_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            skip_all(&mut plan, &candidates, DmSkip::NotOrgMember);
            return plan;
        }
        Err(e) => {
            tracing::warn!(user_id, error = %e, "membership check failed; not sending DMs");
            skip_all(&mut plan, &candidates, DmSkip::ForestUnavailable);
            return plan;
        }
    };
    let prefs = match recipients.slack_preferences(user_id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(user_id, error = %e, "preference read failed; not sending DMs");
            skip_all(&mut plan, &candidates, DmSkip::ForestUnavailable);
            return plan;
        }
    };
    let personal = PersonalPreference::from_explicit(prefs.get(&event.notification_type).copied());

    for (team, link) in &candidates {
        let decision = decide_team(
            personal,
            &team.baseline,
            &event.notification_type,
            link.as_deref(),
            &release.source_attribution,
        );
        let route = match decision {
            TeamDecision::Send(route) => route,
            TeamDecision::NoRoute => continue,
            TeamDecision::Skip(reason) => {
                plan.skips.push(skip(team, &profile, reason));
                continue;
            }
            TeamDecision::NeedsDirectory => {
                let Some(slack) = slack else {
                    plan.skips
                        .push(skip(team, &profile, DmSkip::SlackUnavailable));
                    continue;
                };
                match resolve_directory_match(store, slack, team, &profile, now, false).await {
                    Ok(MappingStatus::Matched { slack_user_id }) => {
                        DmRoute::Baseline { slack_user_id }
                    }
                    Ok(other) => {
                        plan.skips
                            .push(skip(team, &profile, DmSkip::Unmapped(other)));
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(team_id = %team.team_id, error = %e, "slack directory lookup failed");
                        plan.skips
                            .push(skip(team, &profile, DmSkip::SlackUnavailable));
                        continue;
                    }
                }
            }
        };
        tracing::info!(
            user_id,
            team_id = %team.team_id,
            route = match route { DmRoute::Personal { .. } => "personal", DmRoute::Baseline { .. } => "baseline" },
            "DM routing: sending individual notification"
        );
        plan.tasks.push(DispatchTask::SlackDm {
            integration_id: team.integration.id.clone(),
            access_token: team.access_token.clone(),
            slack_user_id: route.slack_user_id().to_string(),
            release_id: release.slug.clone(),
            notification_id: event.id.clone(),
            event_type: event.notification_type.clone(),
            event: event.clone(),
            message: format_slack_message(event, &HashMap::new(), ""),
        });
    }
    plan
}

fn skip(team: &SlackTeam, profile: &RecipientProfile, reason: DmSkip) -> DmSkipRecord {
    DmSkipRecord {
        integration_id: team.integration.id.clone(),
        user_id: profile.user_id.clone(),
        username: profile.username.clone(),
        reason,
    }
}

pub async fn record_skips(store: &dyn IntegrationStore, event: &NotificationEvent, plan: &DmPlan) {
    for s in &plan.skips {
        let already = store
            .list_deliveries(&s.integration_id, 100)
            .await
            .unwrap_or_default()
            .iter()
            .any(|d| d.notification_id == event.id && d.status == DeliveryStatus::Skipped);
        if already {
            continue;
        }
        let who = if s.username.is_empty() {
            &s.user_id
        } else {
            &s.username
        };
        let message = format!("Direct message to {who} skipped: {}", s.reason.describe());
        if let Err(e) = store
            .record_delivery(
                &s.integration_id,
                &event.id,
                DeliveryStatus::Skipped,
                Some(&message),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to record skipped DM");
        }
    }
}

#[cfg(test)]
#[path = "direct_tests.rs"]
mod tests;
