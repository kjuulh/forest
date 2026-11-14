use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use crate::auth::UserEmail;
use crate::integrations::baseline::{CandidateRejection, SlackBaseline};
use crate::integrations::router::ReleaseContext;
use crate::integrations::{
    CreateIntegrationInput, InMemoryIntegrationStore, IntegrationType, SlackUserLink,
};

#[derive(Default)]
struct FakeForest {
    members: HashMap<String, RecipientProfile>,
    prefs: HashMap<String, HashMap<String, bool>>,
    down: bool,
}

#[async_trait::async_trait]
impl RecipientDirectory for FakeForest {
    async fn member(&self, org: &str, user_id: &str) -> Result<Option<RecipientProfile>, String> {
        if self.down {
            return Err("unavailable".into());
        }
        Ok(if org == "acme" {
            self.members.get(user_id).cloned()
        } else {
            None
        })
    }
    async fn slack_preferences(&self, user_id: &str) -> Result<HashMap<String, bool>, String> {
        if self.down {
            return Err("unavailable".into());
        }
        Ok(self.prefs.get(user_id).cloned().unwrap_or_default())
    }
}

#[derive(Default)]
struct FakeSlack {
    users: HashMap<String, SlackDirectoryUser>,
    down: Mutex<bool>,
    calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl SlackDirectory for FakeSlack {
    async fn lookup_by_email(
        &self,
        token: &str,
        email: &str,
    ) -> Result<Option<SlackDirectoryUser>, String> {
        assert_eq!(token, "xoxb-acme");
        self.calls.lock().unwrap().push(email.to_string());
        if *self.down.lock().unwrap() {
            return Err("ratelimited".into());
        }
        Ok(self.users.get(email).cloned())
    }
}

fn slack_user(id: &str, email: &str) -> SlackDirectoryUser {
    SlackDirectoryUser {
        id: id.into(),
        team_id: "T1".into(),
        deleted: false,
        is_bot: false,
        is_restricted: false,
        is_ultra_restricted: false,
        is_stranger: false,
        is_email_confirmed: Some(true),
        email: Some(email.into()),
    }
}

fn member(id: &str, emails: &[(&str, bool)]) -> RecipientProfile {
    RecipientProfile {
        user_id: id.into(),
        username: format!("{id}-name"),
        emails: emails
            .iter()
            .map(|(e, v)| UserEmail {
                email: e.to_string(),
                verified: *v,
            })
            .collect(),
        github_user_ids: vec![],
    }
}

fn event(owner: &str, attribution: &str, kind: &str) -> NotificationEvent {
    NotificationEvent {
        id: format!("n-{owner}-{kind}"),
        notification_type: kind.into(),
        title: "Release".into(),
        body: String::new(),
        organisation: "acme".into(),
        project: "web".into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        release: Some(ReleaseContext {
            slug: format!("rel-{owner}"),
            artifact_id: "a".into(),
            release_intent_id: String::new(),
            destination: "prod".into(),
            environment: "prod".into(),
            source_username: owner.into(),
            source_user_id: owner.into(),
            source_attribution: attribution.into(),
            commit_sha: "abc".into(),
            commit_branch: "main".into(),
            context_title: String::new(),
            context_web: String::new(),
            destination_count: 1,
            error_message: None,
        }),
    }
}

async fn acme_slack(store: &InMemoryIntegrationStore, name: &str, mapping: bool) -> String {
    let i = store
        .create_integration(&CreateIntegrationInput {
            organisation: "acme".into(),
            integration_type: IntegrationType::Slack,
            name: name.into(),
            config: IntegrationConfig::Slack {
                team_id: "T1".into(),
                team_name: "Acme".into(),
                channel_id: "C1".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-acme".into(),
                webhook_url: String::new(),
            },
            created_by: "admin".into(),
        })
        .await
        .unwrap();
    let mut b = SlackBaseline::legacy(&i.id, &[]);
    b.directory_mapping = mapping;
    store.upsert_slack_baseline(&b).await.unwrap();
    i.id
}

fn world() -> (FakeForest, FakeSlack) {
    let mut forest = FakeForest::default();
    forest
        .members
        .insert("alice".into(), member("alice", &[("alice@acme.io", true)]));
    forest
        .members
        .insert("bob".into(), member("bob", &[("bob@acme.io", true)]));
    forest
        .members
        .insert("carol".into(), member("carol", &[("carol@home.io", true)]));
    let mut slack = FakeSlack::default();
    slack.users.insert(
        "alice@acme.io".into(),
        slack_user("UALICE", "alice@acme.io"),
    );
    slack
        .users
        .insert("bob@acme.io".into(), slack_user("UBOB", "bob@acme.io"));
    (forest, slack)
}

async fn plan(
    store: &InMemoryIntegrationStore,
    forest: &FakeForest,
    slack: &FakeSlack,
    e: &NotificationEvent,
) -> DmPlan {
    plan_direct_messages(store, Some(forest), Some(slack), e, chrono::Utc::now()).await
}

fn recipients(p: &DmPlan) -> Vec<String> {
    p.tasks
        .iter()
        .filter_map(|t| match t {
            DispatchTask::SlackDm { slack_user_id, .. } => Some(slack_user_id.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn two_members_without_personal_links_each_get_only_their_own_release() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();

    let a = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;
    let b = plan(
        &store,
        &forest,
        &slack,
        &event("bob", "github_id", "release_succeeded"),
    )
    .await;

    assert_eq!(recipients(&a), vec!["UALICE"]);
    assert_eq!(recipients(&b), vec!["UBOB"]);
    assert!(a.skips.is_empty() && b.skips.is_empty());
}

#[tokio::test]
async fn an_unmapped_member_gets_nothing_and_the_gap_is_recorded() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("carol", "actor", "release_failed"),
    )
    .await;

    assert!(recipients(&p).is_empty());
    assert_eq!(p.skips.len(), 1);
    assert_eq!(p.skips[0].reason, DmSkip::Unmapped(MappingStatus::Unmapped));
}

#[tokio::test]
async fn a_non_member_is_never_messaged_even_with_a_personal_link() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    store
        .upsert_slack_user_link(&SlackUserLink {
            id: "l".into(),
            user_id: "mallory".into(),
            team_id: "T1".into(),
            team_name: "Acme".into(),
            slack_user_id: "UMAL".into(),
            slack_username: "m".into(),
            created_at: String::new(),
        })
        .await
        .unwrap();
    let (forest, slack) = world();

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("mallory", "actor", "release_failed"),
    )
    .await;

    assert!(recipients(&p).is_empty());
    assert_eq!(p.skips[0].reason, DmSkip::NotOrgMember);
}

#[tokio::test]
async fn removing_a_member_stops_their_dms_on_the_next_event() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (mut forest, slack) = world();
    let before = plan(
        &store,
        &forest,
        &slack,
        &event("bob", "actor", "release_failed"),
    )
    .await;
    forest.members.remove("bob");
    let after = plan(
        &store,
        &forest,
        &slack,
        &event("bob", "actor", "release_failed"),
    )
    .await;

    assert_eq!(recipients(&before), vec!["UBOB"]);
    assert!(recipients(&after).is_empty());
    assert_eq!(after.skips[0].reason, DmSkip::NotOrgMember);
}

#[tokio::test]
async fn an_explicit_opt_out_beats_the_org_baseline() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (mut forest, slack) = world();
    forest.prefs.insert(
        "alice".into(),
        HashMap::from([("release_failed".into(), false)]),
    );

    let failed = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;
    let ok = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_succeeded"),
    )
    .await;

    assert!(recipients(&failed).is_empty());
    assert_eq!(failed.skips[0].reason, DmSkip::OptedOut);
    assert_eq!(recipients(&ok), vec!["UALICE"]);
}

#[tokio::test]
async fn an_explicit_opt_in_beats_an_org_default_off() {
    let store = InMemoryIntegrationStore::new();
    let id = acme_slack(&store, "slack", true).await;
    let mut b = store.get_slack_baseline(&id).await.unwrap().unwrap();
    b.dm_defaults.insert("release_started".into(), false);
    store.upsert_slack_baseline(&b).await.unwrap();
    let (mut forest, slack) = world();
    forest.prefs.insert(
        "alice".into(),
        HashMap::from([("release_started".into(), true)]),
    );

    let alice = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_started"),
    )
    .await;
    let bob = plan(
        &store,
        &forest,
        &slack,
        &event("bob", "actor", "release_started"),
    )
    .await;

    assert_eq!(recipients(&alice), vec!["UALICE"]);
    assert!(recipients(&bob).is_empty());
    assert_eq!(bob.skips[0].reason, DmSkip::OrgDefaultOff);
}

#[tokio::test]
async fn a_personal_link_wins_and_two_integrations_on_one_workspace_send_one_dm() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack-a", true).await;
    acme_slack(&store, "slack-b", true).await;
    store
        .upsert_slack_user_link(&SlackUserLink {
            id: "l".into(),
            user_id: "alice".into(),
            team_id: "T1".into(),
            team_name: "Acme".into(),
            slack_user_id: "UALICE_PERSONAL".into(),
            slack_username: "alice".into(),
            created_at: String::new(),
        })
        .await
        .unwrap();
    let (forest, slack) = world();

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;

    assert_eq!(recipients(&p), vec!["UALICE_PERSONAL"]);
    assert!(
        slack.calls.lock().unwrap().is_empty(),
        "no directory lookup when a personal link exists"
    );
}

#[tokio::test]
async fn commit_email_attribution_is_not_enough_for_baseline_delivery() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "email", "release_failed"),
    )
    .await;

    assert!(recipients(&p).is_empty());
    assert_eq!(p.skips[0].reason, DmSkip::WeakAttribution);
}

#[tokio::test]
async fn an_unverified_email_never_maps_to_a_slack_account() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (mut forest, slack) = world();
    forest
        .members
        .insert("eve".into(), member("eve", &[("alice@acme.io", false)]));

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("eve", "actor", "release_failed"),
    )
    .await;

    assert!(recipients(&p).is_empty());
    assert_eq!(
        p.skips[0].reason,
        DmSkip::Unmapped(MappingStatus::NoVerifiedEmail)
    );
    assert!(slack.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn emails_matching_two_slack_accounts_are_ambiguous_and_a_guest_is_refused() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (mut forest, mut slack) = world();
    forest.members.insert(
        "dave".into(),
        member("dave", &[("alice@acme.io", true), ("bob@acme.io", true)]),
    );
    let mut guest = slack_user("UGUEST", "gus@acme.io");
    guest.is_restricted = true;
    slack.users.insert("gus@acme.io".into(), guest);
    forest
        .members
        .insert("gus".into(), member("gus", &[("gus@acme.io", true)]));

    let dave = plan(
        &store,
        &forest,
        &slack,
        &event("dave", "actor", "release_failed"),
    )
    .await;
    let gus = plan(
        &store,
        &forest,
        &slack,
        &event("gus", "actor", "release_failed"),
    )
    .await;

    assert!(recipients(&dave).is_empty());
    assert_eq!(
        dave.skips[0].reason,
        DmSkip::Unmapped(MappingStatus::Ambiguous)
    );
    assert!(recipients(&gus).is_empty());
    assert_eq!(
        gus.skips[0].reason,
        DmSkip::Unmapped(MappingStatus::Rejected {
            reason: CandidateRejection::Guest
        })
    );
}

#[tokio::test]
async fn a_slack_outage_skips_without_caching_and_the_next_event_retries() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();
    *slack.down.lock().unwrap() = true;

    let down = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_started"),
    )
    .await;
    *slack.down.lock().unwrap() = false;
    let up = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;

    assert!(recipients(&down).is_empty());
    assert_eq!(down.skips[0].reason, DmSkip::SlackUnavailable);
    assert_eq!(recipients(&up), vec!["UALICE"]);
}

#[tokio::test]
async fn a_forest_outage_sends_nothing() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (mut forest, slack) = world();
    forest.down = true;

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;

    assert!(recipients(&p).is_empty());
    assert_eq!(p.skips[0].reason, DmSkip::ForestUnavailable);
}

#[tokio::test]
async fn a_cached_match_is_reused_until_the_emails_change() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (mut forest, mut slack) = world();
    plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_started"),
    )
    .await;
    plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;
    assert_eq!(slack.calls.lock().unwrap().len(), 1);

    forest
        .members
        .insert("alice".into(), member("alice", &[("alice@new.io", true)]));
    slack
        .users
        .insert("alice@new.io".into(), slack_user("UALICE2", "alice@new.io"));
    let p = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;
    assert_eq!(recipients(&p), vec!["UALICE2"]);
}

#[tokio::test]
async fn without_baseline_or_links_routing_is_silent_like_before() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", false).await;
    let (forest, slack) = world();

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;

    assert!(p.tasks.is_empty() && p.skips.is_empty());
}

#[tokio::test]
async fn bot_and_annotation_events_have_no_individual_recipient() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();
    let mut bot = event("alice", "", "release_failed");
    bot.release.as_mut().unwrap().source_user_id = String::new();

    assert!(plan(&store, &forest, &slack, &bot).await.tasks.is_empty());
    assert!(
        plan(
            &store,
            &forest,
            &slack,
            &event("alice", "actor", "release_annotated")
        )
        .await
        .tasks
        .is_empty()
    );
}

#[tokio::test]
async fn a_replayed_old_event_sends_nothing() {
    let store = InMemoryIntegrationStore::new();
    acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();
    let mut old = event("alice", "actor", "release_failed");
    old.timestamp = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();

    let p = plan(&store, &forest, &slack, &old).await;

    assert!(recipients(&p).is_empty());
    assert_eq!(p.skips[0].reason, DmSkip::Stale);
}

#[tokio::test]
async fn a_disabled_integration_sends_no_dm() {
    let store = InMemoryIntegrationStore::new();
    let id = acme_slack(&store, "slack", true).await;
    store
        .set_integration_enabled("acme", &id, false)
        .await
        .unwrap();
    let (forest, slack) = world();

    let p = plan(
        &store,
        &forest,
        &slack,
        &event("alice", "actor", "release_failed"),
    )
    .await;

    assert!(p.tasks.is_empty());
}

#[tokio::test]
async fn skips_appear_in_the_delivery_history_without_provider_data() {
    let store = InMemoryIntegrationStore::new();
    let id = acme_slack(&store, "slack", true).await;
    let (forest, slack) = world();
    let e = event("carol", "actor", "release_failed");
    let p = plan(&store, &forest, &slack, &e).await;

    record_skips(&store, &e, &p).await;

    let history = store.list_deliveries(&id, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, DeliveryStatus::Skipped);
    let msg = history[0].error_message.as_deref().unwrap();
    assert!(msg.contains("carol-name"));
    assert!(
        !msg.contains("carol@home.io"),
        "no email in the history: {msg}"
    );
}
