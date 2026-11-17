use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use forage_core::auth::UserEmail;
use forage_core::auth::oauth_state::{InMemoryOAuthStateStore, OAuthStateStore};
use forage_core::integrations::baseline::SlackBaseline;
use forage_core::integrations::direct::{RecipientDirectory, RecipientProfile};
use forage_core::integrations::nats::NotificationEnvelope;
use forage_core::integrations::router::{NotificationEvent, ReleaseContext};
use forage_core::integrations::signing::sign_hex;
use forage_core::integrations::{
    CreateIntegrationInput, DeliveryStatus, InMemoryIntegrationStore, IntegrationConfig,
    IntegrationStore, IntegrationType, SlackUserLink,
};
use forage_core::platform::{NotificationPreference, OrgMember};
use forage_core::session::{
    CachedOrg, CachedUser, InMemorySessionStore, SessionData, SessionStore,
};
use serde_json::json;
use tower::ServiceExt;

use crate::notification_consumer::NotificationConsumer;
use crate::notification_worker::NotificationDispatcher;
use crate::test_providers::FakeProviders;
use crate::test_support::*;

#[derive(Default)]
struct FakeForest {
    members: Mutex<HashMap<String, RecipientProfile>>,
    prefs: Mutex<HashMap<String, HashMap<String, bool>>>,
}

impl FakeForest {
    fn add(&self, id: &str, email: &str, github: Option<i64>) {
        self.members.lock().unwrap().insert(
            id.into(),
            RecipientProfile {
                user_id: id.into(),
                username: format!("{id}-name"),
                emails: vec![UserEmail {
                    email: email.into(),
                    verified: true,
                }],
                github_user_ids: github.into_iter().collect(),
            },
        );
    }
}

#[async_trait::async_trait]
impl RecipientDirectory for FakeForest {
    async fn member(&self, org: &str, user_id: &str) -> Result<Option<RecipientProfile>, String> {
        Ok(if org == "testorg" {
            self.members.lock().unwrap().get(user_id).cloned()
        } else {
            None
        })
    }
    async fn slack_preferences(&self, user_id: &str) -> Result<HashMap<String, bool>, String> {
        Ok(self
            .prefs
            .lock()
            .unwrap()
            .get(user_id)
            .cloned()
            .unwrap_or_default())
    }
}

struct World {
    app: axum::Router,
    sessions: Arc<InMemorySessionStore>,
    store: Arc<InMemoryIntegrationStore>,
    states: Arc<InMemoryOAuthStateStore>,
    providers: FakeProviders,
    forest: Arc<FakeForest>,
    platform: Arc<MockPlatformClient>,
}

const SIGNING: &str = "slack-signing-secret";
const HOOK: &str = "github-hook-secret";

async fn world_with(platform: MockPlatformClient) -> World {
    let providers = FakeProviders::start().await;
    let sessions = Arc::new(InMemorySessionStore::new());
    let store = Arc::new(InMemoryIntegrationStore::new());
    let states = Arc::new(InMemoryOAuthStateStore::new());
    let forest = Arc::new(FakeForest::default());
    let platform = Arc::new(platform);
    let state = crate::state::AppState::new(
        make_templates(),
        Arc::new(MockForestClient::new()),
        platform.clone(),
        sessions.clone(),
    )
    .with_oauth_state_store(states.clone())
    .with_integration_store(store.clone())
    .with_slack_config(crate::state::SlackConfig {
        client_id: "slack-client".into(),
        client_secret: "slack-secret".into(),
        redirect_host: "http://forage.test".into(),
    })
    .with_slack_api(
        providers.slack_api(),
        format!("{}/slack/oauth/v2/authorize", providers.base),
    )
    .with_slack_signing_secret(SIGNING.into())
    .with_recipient_directory(forest.clone())
    .with_github_app(Arc::new(providers.github_app(HOOK)))
    .with_forage_host("http://forage.test".into());
    World {
        app: crate::build_router(state),
        sessions,
        store,
        states,
        providers,
        forest,
        platform,
    }
}

async fn world() -> World {
    world_with(MockPlatformClient::new()).await
}

async fn session_as(
    sessions: &Arc<InMemorySessionStore>,
    user_id: &str,
    role: &str,
    org: &str,
) -> String {
    let now = Utc::now();
    let id = sessions
        .create(SessionData {
            access_token: "mock-access".into(),
            refresh_token: "mock-refresh".into(),
            csrf_token: "test-csrf".into(),
            needs_username: false,
            access_expires_at: now + chrono::Duration::hours(1),
            user: Some(CachedUser {
                user_id: user_id.into(),
                username: format!("{user_id}-name"),
                profile_picture_url: None,
                emails: vec![UserEmail {
                    email: format!("{user_id}@acme.io"),
                    verified: true,
                }],
                orgs: vec![CachedOrg {
                    organisation_id: "org-1".into(),
                    name: org.into(),
                    role: role.into(),
                }],
            }),
            created_at: now,
            last_seen_at: now,
        })
        .await
        .unwrap();
    format!("forage_session={id}")
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, String, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let location = resp
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap()
}

fn post(uri: &str, cookie: &str, form: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .unwrap()
}

async fn slack_integration(
    store: &InMemoryIntegrationStore,
    org: &str,
    team: &str,
    name: &str,
) -> String {
    store
        .create_integration(&CreateIntegrationInput {
            organisation: org.into(),
            integration_type: IntegrationType::Slack,
            name: name.into(),
            config: IntegrationConfig::Slack {
                team_id: team.into(),
                team_name: "Acme".into(),
                channel_id: "C-DEPLOYS".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-acme".into(),
                webhook_url: String::new(),
            },
            created_by: "admin".into(),
        })
        .await
        .unwrap()
        .id
}

async fn enable_baseline(store: &InMemoryIntegrationStore, id: &str) {
    let mut b = SlackBaseline::legacy(id, &[]);
    b.directory_mapping = true;
    store.upsert_slack_baseline(&b).await.unwrap();
}

// --- baseline settings ---------------------------------------------------

#[tokio::test]
async fn an_admin_turns_on_the_baseline_and_a_member_cannot() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;
    let member = session_as(&w.sessions, "bob", "member", "testorg").await;
    let form = "_csrf=test-csrf&directory_mapping=on&dm_release_failed=on&dm_release_succeeded=on";

    let (status, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{id}/baseline"),
            &member,
            form,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(w.store.get_slack_baseline(&id).await.unwrap().is_none());

    let (status, location, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{id}/baseline"),
            &admin,
            form,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.ends_with("?saved=1"));
    let b = w.store.get_slack_baseline(&id).await.unwrap().unwrap();
    assert!(b.directory_mapping);
    assert!(!b.org_default("release_started"));
    assert!(b.org_default("release_failed"));
    assert_eq!(b.updated_by, "admin-name");
}

#[tokio::test]
async fn baseline_settings_refuse_csrf_mismatch_other_orgs_and_webhooks() {
    let w = world().await;
    let theirs = slack_integration(&w.store, "otherorg", "T9", "slack").await;
    let ours = slack_integration(&w.store, "testorg", "T1", "slack").await;
    let hook = w
        .store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "hook".into(),
            config: IntegrationConfig::Webhook {
                url: "https://x.test".into(),
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "admin".into(),
        })
        .await
        .unwrap()
        .id;
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    let (s, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{ours}/baseline"),
            &admin,
            "_csrf=wrong&directory_mapping=on",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/otherorg/settings/integrations/{theirs}/baseline"),
            &admin,
            "_csrf=test-csrf&directory_mapping=on",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "not a member of otherorg");
    let (s, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{theirs}/baseline"),
            &admin,
            "_csrf=test-csrf&directory_mapping=on",
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "another org's integration looks absent"
    );
    let (s, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{hook}/baseline"),
            &admin,
            "_csrf=test-csrf&directory_mapping=on",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(w.store.get_slack_baseline(&theirs).await.unwrap().is_none());
}

#[tokio::test]
async fn the_detail_page_shows_the_baseline_and_skipped_deliveries() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    w.store
        .record_delivery(
            &id,
            "n1",
            DeliveryStatus::Skipped,
            Some("Direct message to carol skipped: no Slack account matches a verified email"),
        )
        .await
        .unwrap();
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    let (s, _, body) = send(
        &w.app,
        get(&format!("/orgs/testorg/settings/integrations/{id}"), &admin),
    )
    .await;

    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("Reach members without a personal Slack link"));
    assert!(body.contains("Member coverage"));
    assert!(body.contains("Skipped"));
}

// --- coverage --------------------------------------------------------------

fn members(list: &[(&str, &str)]) -> MockPlatformClient {
    MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_members_result: Some(Ok(list
            .iter()
            .map(|(id, role)| OrgMember {
                user_id: id.to_string(),
                username: format!("{id}-name"),
                role: role.to_string(),
                joined_at: None,
            })
            .collect())),
        ..Default::default()
    })
}

#[tokio::test]
async fn coverage_shows_each_member_route_without_leaking_the_directory() {
    let w = world_with(members(&[
        ("alice", "member"),
        ("bob", "member"),
        ("carol", "member"),
        ("dave", "member"),
    ]))
    .await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.forest.add("alice", "alice@acme.io", None);
    w.forest.add("bob", "bob@acme.io", None);
    w.forest.add("carol", "carol@home.io", None);
    w.forest.add("dave", "dave@acme.io", None);
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    w.providers
        .add_slack_user("dave@acme.io", "UDAVE", "T-OTHER");
    w.store
        .upsert_slack_user_link(&SlackUserLink {
            id: "l".into(),
            user_id: "bob".into(),
            team_id: "T1".into(),
            team_name: "Acme".into(),
            slack_user_id: "UBOB".into(),
            slack_username: "bob".into(),
            created_at: String::new(),
        })
        .await
        .unwrap();
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    let (s, _, body) = send(
        &w.app,
        get(
            &format!("/orgs/testorg/settings/integrations/{id}/coverage"),
            &admin,
        ),
    )
    .await;

    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("Matched by verified Forest email"));
    assert!(body.contains("Personal Slack link"));
    assert!(body.contains("No Slack account matches a verified email"));
    assert!(body.contains("belongs to another workspace"));
    assert!(
        body.contains(r#"data-testid="coverage-reachable">2<"#),
        "{body}"
    );
    for secret in [
        "UALICE",
        "UDAVE",
        "alice@acme.io",
        "carol@home.io",
        "xoxb-acme",
    ] {
        assert!(!body.contains(secret), "coverage leaked {secret}");
    }

    let member = session_as(&w.sessions, "bob", "member", "testorg").await;
    let (s, _, _) = send(
        &w.app,
        get(
            &format!("/orgs/testorg/settings/integrations/{id}/coverage"),
            &member,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// --- Slack install state ---------------------------------------------------

async fn seed_slack_state(
    states: &InMemoryOAuthStateStore,
    nonce: &str,
    user: &str,
    reinstall: &str,
) {
    states
        .create(
            "slack_install",
            nonce,
            Some(&format!("testorg\n{user}\n{reinstall}")),
            Utc::now() + chrono::Duration::minutes(5),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn starting_a_slack_install_binds_a_single_use_nonce() {
    let w = world().await;
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    let (s, location, _) = send(
        &w.app,
        post(
            "/orgs/testorg/settings/integrations/install/slack/start",
            &admin,
            "_csrf=test-csrf",
        ),
    )
    .await;

    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(location.contains("users:read.email"));
    assert!(
        !location.contains("state=testorg"),
        "the org name is no longer the state"
    );
    let nonce = location.split("state=").nth(1).unwrap();
    assert_eq!(nonce.len(), 64);
}

#[tokio::test]
async fn a_forged_foreign_or_replayed_slack_state_is_refused() {
    let w = world().await;
    w.providers.state.lock().unwrap().oauth_access =
        json!({ "ok": true, "team": {"id": "T1", "name": "Acme"}, "access_token": "xoxb-new" });
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;
    let other_admin = session_as(&w.sessions, "mallory", "admin", "testorg").await;

    let (s, _, _) = send(
        &w.app,
        get("/integrations/slack/callback?code=c&state=testorg", &admin),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "bare org name");

    seed_slack_state(&w.states, "n-admin", "admin", "").await;
    let (s, _, _) = send(
        &w.app,
        get(
            "/integrations/slack/callback?code=c&state=n-admin",
            &other_admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "started by someone else");
    let (s, _, _) = send(
        &w.app,
        get("/integrations/slack/callback?code=c&state=n-admin", &admin),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "nonce was consumed by the refused attempt"
    );

    seed_slack_state(&w.states, "n-ok", "admin", "").await;
    let (s, _, _) = send(
        &w.app,
        get("/integrations/slack/callback?code=c&state=n-ok", &admin),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = send(
        &w.app,
        get("/integrations/slack/callback?code=c&state=n-ok", &admin),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "replay");
    assert_eq!(w.store.list_integrations("testorg").await.unwrap().len(), 1);
}

#[tokio::test]
async fn slack_installs_refuse_grid_org_installs_and_a_reinstall_into_another_workspace() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    w.providers.state.lock().unwrap().oauth_access = json!({ "ok": true, "is_enterprise_install": true, "team": null, "access_token": "xoxb-grid" });
    seed_slack_state(&w.states, "n1", "admin", "").await;
    let (s, location, _) = send(
        &w.app,
        get("/integrations/slack/callback?code=c&state=n1", &admin),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(location.contains("Enterprise"));

    w.providers.state.lock().unwrap().oauth_access =
        json!({ "ok": true, "team": {"id": "T2", "name": "Evil"}, "access_token": "xoxb-evil" });
    seed_slack_state(&w.states, "n2", "admin", &id).await;
    let (s, location, _) = send(
        &w.app,
        get("/integrations/slack/callback?code=c&state=n2", &admin),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(location.contains("different%20Slack%20workspace"));
    let unchanged = w.store.get_integration("testorg", &id).await.unwrap();
    assert!(
        matches!(unchanged.config, IntegrationConfig::Slack { ref access_token, .. } if access_token == "xoxb-acme")
    );
}

#[tokio::test]
async fn slack_callback_error_redirects_back_to_the_install_page() {
    let w = world().await;
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;
    seed_slack_state(&w.states, "n", "admin", "").await;

    let (s, location, _) = send(
        &w.app,
        get(
            "/integrations/slack/callback?state=n&error=access_denied",
            &admin,
        ),
    )
    .await;

    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(location.contains("install/slack") && location.contains("error="));
    assert!(!location.contains("access_denied"));
}

// --- Slack events ----------------------------------------------------------

fn slack_event(body: &str, ts: i64, secret: &str) -> Request<Body> {
    let sig = format!(
        "v0={}",
        sign_hex(secret, format!("v0:{ts}:{body}").as_bytes())
    );
    Request::builder()
        .method("POST")
        .uri("/integrations/slack/events")
        .header("x-slack-request-timestamp", ts.to_string())
        .header("x-slack-signature", sig)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn an_uninstall_event_disables_only_that_workspace() {
    let w = world().await;
    let ours = slack_integration(&w.store, "testorg", "T1", "slack").await;
    let theirs = slack_integration(&w.store, "otherorg", "T2", "slack").await;
    let now = Utc::now().timestamp();
    let body = r#"{"type":"event_callback","team_id":"T1","event":{"type":"app_uninstalled"}}"#;

    let (s, _, _) = send(&w.app, slack_event(body, now, "wrong-secret")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = send(&w.app, slack_event(body, now - 600, SIGNING)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "stale timestamp is a replay");
    assert!(
        w.store
            .get_integration("testorg", &ours)
            .await
            .unwrap()
            .enabled
    );

    let (s, _, _) = send(&w.app, slack_event(body, now, SIGNING)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        !w.store
            .get_integration("testorg", &ours)
            .await
            .unwrap()
            .enabled
    );
    assert!(
        w.store
            .get_integration("otherorg", &theirs)
            .await
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn the_slack_url_verification_challenge_is_echoed_only_when_signed() {
    let w = world().await;
    let body = r#"{"type":"url_verification","challenge":"abc123"}"#;
    let (s, _, text) = send(&w.app, slack_event(body, Utc::now().timestamp(), SIGNING)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(text.contains("abc123"));
}

// --- notification routing through the real dispatcher -------------------

fn release_event(owner: &str, attribution: &str, kind: &str, n: &str) -> Vec<u8> {
    let event = NotificationEvent {
        id: n.into(),
        notification_type: kind.into(),
        title: "Release".into(),
        body: String::new(),
        organisation: "testorg".into(),
        project: "web".into(),
        timestamp: Utc::now().to_rfc3339(),
        release: Some(ReleaseContext {
            slug: format!("rel-{owner}"),
            artifact_id: "a".into(),
            release_intent_id: String::new(),
            destination: "prod".into(),
            environment: "prod".into(),
            source_username: owner.into(),
            source_user_id: owner.into(),
            source_attribution: attribution.into(),
            commit_sha: "abc1234".into(),
            commit_branch: "main".into(),
            context_title: String::new(),
            context_web: String::new(),
            destination_count: 1,
            error_message: None,
        }),
    };
    serde_json::to_vec(&NotificationEnvelope::from(&event)).unwrap()
}

async fn deliver(w: &World, payload: &[u8]) {
    let dispatcher = NotificationDispatcher::new(w.store.clone(), String::new())
        .with_slack_api(w.providers.slack_api());
    let slack = w.providers.slack_api();
    NotificationConsumer::process_payload(
        payload,
        w.store.as_ref(),
        Some(w.forest.as_ref()),
        Some(&slack),
        &dispatcher,
    )
    .await
    .unwrap();
}

fn dms(w: &World) -> Vec<String> {
    w.providers
        .dm_recipients()
        .into_iter()
        .filter(|c| c.starts_with('U'))
        .collect()
}

#[tokio::test]
async fn two_members_without_personal_links_get_only_their_own_dm_over_http() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.forest.add("alice", "alice@acme.io", None);
    w.forest.add("bob", "bob@acme.io", None);
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    w.providers.add_slack_user("bob@acme.io", "UBOB", "T1");

    deliver(&w, &release_event("alice", "actor", "release_failed", "n1")).await;
    assert_eq!(dms(&w), vec!["UALICE"]);
    deliver(
        &w,
        &release_event("bob", "github_id", "release_succeeded", "n2"),
    )
    .await;
    assert_eq!(dms(&w), vec!["UALICE", "UBOB"]);
    let channel_posts = w
        .providers
        .dm_recipients()
        .iter()
        .filter(|c| *c == "C-DEPLOYS")
        .count();
    assert_eq!(channel_posts, 2, "channel posts are a separate delivery");
}

#[tokio::test]
async fn retries_and_redeliveries_update_the_same_dm_instead_of_duplicating() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.forest.add("alice", "alice@acme.io", None);
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    let payload = release_event("alice", "actor", "release_started", "n1");

    deliver(&w, &payload).await;
    deliver(&w, &payload).await;
    deliver(
        &w,
        &release_event("alice", "actor", "release_succeeded", "n2"),
    )
    .await;

    assert_eq!(dms(&w), vec!["UALICE"], "one DM per release");
    let updates = w
        .providers
        .state
        .lock()
        .unwrap()
        .updated
        .iter()
        .filter(|(c, _)| c == "UALICE")
        .count();
    assert_eq!(updates, 2);
}

#[tokio::test]
async fn opted_out_unmapped_and_non_members_get_nothing_and_skips_are_recorded() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.forest.add("alice", "alice@acme.io", None);
    w.forest.add("carol", "carol@home.io", None);
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    w.providers.add_slack_user("mallory@acme.io", "UMAL", "T1");
    w.forest.prefs.lock().unwrap().insert(
        "alice".into(),
        HashMap::from([("release_failed".into(), false)]),
    );

    deliver(&w, &release_event("alice", "actor", "release_failed", "n1")).await;
    deliver(&w, &release_event("carol", "actor", "release_failed", "n2")).await;
    deliver(
        &w,
        &release_event("mallory", "actor", "release_failed", "n3"),
    )
    .await;

    assert!(dms(&w).is_empty(), "got {:?}", dms(&w));
    let skipped: Vec<_> = w
        .store
        .list_deliveries(&id, 50)
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.status == DeliveryStatus::Skipped)
        .filter_map(|d| d.error_message)
        .collect();
    assert_eq!(skipped.len(), 3, "{skipped:?}");
    assert!(
        skipped
            .iter()
            .any(|m| m.contains("turned Slack notifications off"))
    );
    assert!(
        skipped
            .iter()
            .any(|m| m.contains("no Slack account matches"))
    );
    assert!(skipped.iter().any(|m| m.contains("not a current member")));
}

#[tokio::test]
async fn a_revoked_token_disables_the_integration_and_stops_routing() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.forest.add("alice", "alice@acme.io", None);
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    w.providers.state.lock().unwrap().post_error = Some("token_revoked".into());

    deliver(&w, &release_event("alice", "actor", "release_failed", "n1")).await;
    assert!(
        !w.store
            .get_integration("testorg", &id)
            .await
            .unwrap()
            .enabled
    );

    w.providers.state.lock().unwrap().post_error = None;
    deliver(&w, &release_event("alice", "actor", "release_failed", "n2")).await;
    assert!(w.providers.dm_recipients().is_empty());
}

// --- personal settings -----------------------------------------------------

#[tokio::test]
async fn the_account_page_shows_inherited_and_overridden_settings_and_resets() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_notification_preferences_result: Some(Ok(vec![NotificationPreference {
            notification_type: "NOTIFICATION_TYPE_RELEASE_FAILED".into(),
            channel: "NOTIFICATION_CHANNEL_SLACK".into(),
            enabled: false,
        }])),
        ..Default::default()
    });
    let w = world_with(platform).await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    let alice = session_as(&w.sessions, "alice", "member", "testorg").await;

    let (s, _, body) = send(&w.app, get("/settings/account", &alice)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("Individual Slack messages"));
    assert!(
        body.contains("Off &middot; your choice (organisation default: on)"),
        "{body}"
    );
    assert!(
        body.contains("Following your organisation&#x27;s default (on)")
            || body.contains("Following your organisation's default (on)")
    );
    assert!(body.contains("matching your verified email"));
    assert!(!body.contains("UALICE"));

    let (s, _, _) = send(&w.app, post("/settings/account/notifications/reset", &alice, "_csrf=test-csrf&notification_type=NOTIFICATION_TYPE_RELEASE_FAILED&channel=NOTIFICATION_CHANNEL_SLACK")).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let (s, _, _) = send(&w.app, post("/settings/account/notifications/reset", &alice, "_csrf=bad&notification_type=NOTIFICATION_TYPE_RELEASE_FAILED&channel=NOTIFICATION_CHANNEL_SLACK")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(
        w.platform.preference_calls(),
        vec![(
            "NOTIFICATION_TYPE_RELEASE_FAILED".to_string(),
            "NOTIFICATION_CHANNEL_SLACK".to_string(),
            None
        )]
    );
}

#[tokio::test]
async fn the_account_page_hides_individual_settings_without_a_workspace() {
    let w = world().await;
    let alice = session_as(&w.sessions, "alice", "member", "testorg").await;
    let (_, _, body) = send(&w.app, get("/settings/account", &alice)).await;
    assert!(!body.contains("Individual Slack messages"));
}

// --- GitHub organisation connection ---------------------------------------

fn github_ready(w: &World) {
    let mut st = w.providers.state.lock().unwrap();
    st.gh_user = json!({ "id": 42, "login": "octo-owner" });
    st.gh_user_installations = vec![777];
    st.gh_installations.insert(
        777,
        json!({ "id": 777, "account": {"id": 9000, "login": "acme-gh", "type": "Organization"},
                "suspended_at": null, "permissions": {"members": "read"} }),
    );
    st.gh_memberships.insert(
        ("acme-gh".into(), "octo-owner".into()),
        json!({"role": "admin", "state": "active"}),
    );
}

async fn seed_verify(
    states: &InMemoryOAuthStateStore,
    nonce: &str,
    org: &str,
    user: &str,
    installation: i64,
) {
    states
        .create(
            "github_install_verify",
            nonce,
            Some(&format!("{org}\n{user}\n{installation}")),
            Utc::now() + chrono::Duration::minutes(5),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn the_github_install_round_trip_binds_one_installation_to_one_org() {
    let w = world().await;
    github_ready(&w);
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    let (s, location, _) = send(
        &w.app,
        post(
            "/orgs/testorg/settings/integrations/github",
            &admin,
            "_csrf=test-csrf",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(location.contains("/apps/forest-test/installations/new?state="));
    let nonce = location.split("state=").nth(1).unwrap().to_string();

    let (s, location, _) = send(
        &w.app,
        get(
            &format!(
                "/integrations/github/setup?installation_id=777&setup_action=install&state={nonce}"
            ),
            &admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(location.contains("/login/oauth/authorize?client_id=Iv1.test"));
    let verify = location.split("state=").nth(1).unwrap().to_string();

    let (s, location, _) = send(
        &w.app,
        get(
            &format!("/integrations/github/callback?code=good-code&state={verify}"),
            &admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER, "{location}");
    let integrations = w.store.list_integrations("testorg").await.unwrap();
    assert_eq!(integrations.len(), 1);
    assert_eq!(integrations[0].integration_type, IntegrationType::GitHub);
    assert_eq!(
        w.store.get_github_installation_binding(777).await.unwrap(),
        Some(("testorg".to_string(), integrations[0].id.clone()))
    );

    let other = session_as(&w.sessions, "eve", "admin", "otherorg").await;
    seed_verify(&w.states, "v-other", "otherorg", "eve", 777).await;
    let (s, _, body) = send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v-other",
            &other,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(body.contains("already connected to another Forest organisation"));
    assert!(
        w.store
            .list_integrations("otherorg")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn github_installs_are_refused_unless_owned_by_the_installing_org_owner() {
    let w = world().await;
    github_ready(&w);
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;

    seed_verify(&w.states, "v1", "testorg", "admin", 778).await;
    let (s, _, body) = send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v1",
            &admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(body.contains("not accessible"), "spoofed installation id");

    w.providers.state.lock().unwrap().gh_memberships.clear();
    seed_verify(&w.states, "v2", "testorg", "admin", 777).await;
    let (s, _, body) = send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v2",
            &admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(body.contains("Only an owner"));

    github_ready(&w);
    w.providers
        .state
        .lock()
        .unwrap()
        .gh_installations
        .get_mut(&777)
        .unwrap()["account"]["type"] = json!("User");
    seed_verify(&w.states, "v3", "testorg", "admin", 777).await;
    let (s, _, _) = send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v3",
            &admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let member = session_as(&w.sessions, "bob", "member", "testorg").await;
    seed_verify(&w.states, "v4", "testorg", "bob", 777).await;
    let (s, _, _) = send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v4",
            &member,
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "a Forest member who is not an admin"
    );

    seed_verify(&w.states, "v5", "testorg", "admin", 777).await;
    let (s, _, _) = send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v5",
            &member,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "someone else's flow");
    assert!(
        w.store
            .list_integrations("testorg")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_github_connection_page_shows_attribution_coverage() {
    let w = world_with(members(&[
        ("alice", "member"),
        ("bob", "member"),
        ("carol", "member"),
    ]))
    .await;
    github_ready(&w);
    w.providers.state.lock().unwrap().gh_members = vec![1001];
    w.forest.add("alice", "alice@acme.io", Some(1001));
    w.forest.add("bob", "bob@acme.io", Some(2002));
    w.forest.add("carol", "carol@acme.io", None);
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;
    seed_verify(&w.states, "v", "testorg", "admin", 777).await;
    send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v",
            &admin,
        ),
    )
    .await;
    let id = w.store.list_integrations("testorg").await.unwrap()[0]
        .id
        .clone();

    let (s, _, body) = send(
        &w.app,
        get(&format!("/orgs/testorg/settings/integrations/{id}"), &admin),
    )
    .await;

    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("Healthy"));
    assert!(body.contains("Linked GitHub account is a member of acme-gh"));
    assert!(body.contains("Linked GitHub account is not a member of acme-gh"));
    assert!(body.contains("No GitHub account linked to Forest"));
}

fn github_hook(event: &str, body: &str, secret: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/integrations/github/webhook")
        .header("x-github-event", event)
        .header("x-github-delivery", "d-1")
        .header(
            "x-hub-signature-256",
            format!("sha256={}", sign_hex(secret, body.as_bytes())),
        )
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn github_installation_webhooks_suspend_and_remove_the_connection() {
    let w = world().await;
    github_ready(&w);
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;
    seed_verify(&w.states, "v", "testorg", "admin", 777).await;
    send(
        &w.app,
        get(
            "/integrations/github/callback?code=good-code&state=v",
            &admin,
        ),
    )
    .await;
    let id = w.store.list_integrations("testorg").await.unwrap()[0]
        .id
        .clone();

    let suspend = r#"{"action":"suspend","installation":{"id":777}}"#;
    let (s, _, _) = send(&w.app, github_hook("installation", suspend, "wrong")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = send(&w.app, github_hook("installation", suspend, HOOK)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(
        !w.store
            .get_integration("testorg", &id)
            .await
            .unwrap()
            .enabled
    );

    let deleted = r#"{"action":"deleted","installation":{"id":777}}"#;
    send(&w.app, github_hook("installation", deleted, HOOK)).await;
    assert!(
        w.store
            .list_integrations("testorg")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        w.store.get_github_installation_binding(777).await.unwrap(),
        None
    );
}

// --- review regressions ----------------------------------------------------

#[tokio::test]
async fn only_the_sending_integration_of_a_workspace_takes_baseline_settings() {
    let w = world_with(members(&[("alice", "member")])).await;
    let sender = slack_integration(&w.store, "testorg", "T1", "deploys").await;
    let other = slack_integration(&w.store, "testorg", "T1", "releases").await;
    let admin = session_as(&w.sessions, "admin", "admin", "testorg").await;
    let form = "_csrf=test-csrf&directory_mapping=on";

    let (s, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{other}/baseline"),
            &admin,
            form,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(w.store.get_slack_baseline(&other).await.unwrap().is_none());
    let (s, _, _) = send(
        &w.app,
        get(
            &format!("/orgs/testorg/settings/integrations/{other}/coverage"),
            &admin,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (_, _, body) = send(
        &w.app,
        get(
            &format!("/orgs/testorg/settings/integrations/{other}"),
            &admin,
        ),
    )
    .await;
    assert!(body.contains("configured on"), "{body}");

    let (s, _, _) = send(
        &w.app,
        post(
            &format!("/orgs/testorg/settings/integrations/{sender}/baseline"),
            &admin,
            form,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn a_revoked_token_disables_every_integration_on_that_workspace() {
    let w = world().await;
    let a = slack_integration(&w.store, "testorg", "T1", "deploys").await;
    let b = slack_integration(&w.store, "testorg", "T1", "releases").await;
    let other = slack_integration(&w.store, "testorg", "T2", "elsewhere").await;
    enable_baseline(&w.store, &a).await;
    w.forest.add("alice", "alice@acme.io", None);
    w.providers.add_slack_user("alice@acme.io", "UALICE", "T1");
    w.providers.state.lock().unwrap().post_error = Some("invalid_auth".into());

    deliver(&w, &release_event("alice", "actor", "release_failed", "n1")).await;

    assert!(
        !w.store
            .get_integration("testorg", &a)
            .await
            .unwrap()
            .enabled
    );
    assert!(
        !w.store
            .get_integration("testorg", &b)
            .await
            .unwrap()
            .enabled
    );
    assert!(
        !w.store
            .get_integration("testorg", &other)
            .await
            .unwrap()
            .enabled,
        "T2 failed its own channel post with invalid_auth too"
    );
}

#[tokio::test]
async fn a_redelivered_event_records_its_skip_once() {
    let w = world().await;
    let id = slack_integration(&w.store, "testorg", "T1", "slack").await;
    enable_baseline(&w.store, &id).await;
    w.forest.add("carol", "carol@home.io", None);
    let payload = release_event("carol", "actor", "release_failed", "n1");

    deliver(&w, &payload).await;
    deliver(&w, &payload).await;

    let skips = w
        .store
        .list_deliveries(&id, 50)
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.status == DeliveryStatus::Skipped)
        .count();
    assert_eq!(skips, 1);
}

#[tokio::test]
async fn a_personal_slack_link_needs_the_nonce_from_its_own_start() {
    let w = world().await;
    let alice = session_as(&w.sessions, "alice", "member", "testorg").await;

    let (s, _, _) = send(
        &w.app,
        get(
            "/settings/account/slack/callback?code=c&state=alice",
            &alice,
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "the user id is no longer a valid state"
    );

    let (s, location, _) = send(&w.app, get("/settings/account/slack/connect", &alice)).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let nonce = location.split("state=").nth(1).unwrap().to_string();
    assert_eq!(nonce.len(), 64);
    let bob = session_as(&w.sessions, "bob", "member", "testorg").await;
    let (s, _, _) = send(
        &w.app,
        get(
            &format!("/settings/account/slack/callback?code=c&state={nonce}"),
            &bob,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "someone else's nonce");
    assert!(
        w.store
            .list_slack_user_links("bob")
            .await
            .unwrap()
            .is_empty()
    );
}
