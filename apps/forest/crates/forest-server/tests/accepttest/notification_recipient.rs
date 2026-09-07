//! A personal release notification goes to the release's owner and to nobody
//! else (DATA-723).
//!
//! The feed used to be an org-wide broadcast: `poll_notifications` filtered on
//! organisation, project and opt-out preferences and nothing more, so every
//! member of the org received a personal notification for every release in it —
//! including a `renovate[bot]` dependency bump nobody had written. Everyone
//! already sees those in the shared release channel; a personal copy of
//! somebody else's release is noise with a person's name on it.
//!
//! The channel is fed by forage's listener under the service-account key, and
//! that feed is deliberately unfiltered. These tests pin both halves: a person
//! sees their own releases, and the service account still sees all of them.

use forest_grpc_interface::*;

use crate::accepttest::fixtures::{
    Fixture, RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, fixture, restricted_fixture,
};

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: tonic::metadata::MetadataValue<_> =
        format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

/// A registered user, with the token they read their feed with and the id a
/// release records as its owner.
struct Person {
    user_id: uuid::Uuid,
    token: String,
}

async fn register(fixture: &Fixture) -> Person {
    let resp = fixture
        .users()
        .register(RegisterRequest {
            username: format!("notif-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user")
        .into_inner();

    Person {
        user_id: resp.user.expect("user").user_id.parse().expect("user id"),
        token: resp.tokens.expect("tokens").access_token,
    }
}

/// Insert a notification the way `create_notification` does, with `owner` as
/// the release's personal recipient — `None` for a release nobody owns, which
/// is what `release_owner` produces for a bot's.
///
/// Written straight to the table: the gate under test is on the read path, and
/// driving it through an annotate + release round-trip per case would prove
/// nothing extra about who the row reaches.
async fn seed(db: &sqlx::PgPool, org: &str, project: &str, title: &str, owner: Option<uuid::Uuid>) {
    let context = serde_json::json!({
        "slug": format!("rel-{}", uuid::Uuid::now_v7()),
        "source_username": "whoever-wrote-it",
        "source_user_id": owner.map(|o| o.to_string()),
    });

    sqlx::query(
        "INSERT INTO notifications (notification_type, title, body, organisation, project, release_context)
         VALUES ('RELEASE_SUCCEEDED', $1, 'Release succeeded', $2, $3, $4)",
    )
    .bind(title)
    .bind(org)
    .bind(project)
    .bind(context)
    .execute(db)
    .await
    .expect("seed notification");
}

async fn titles_for(fixture: &Fixture, token: &str, org: &str, project: &str) -> Vec<String> {
    let resp = fixture
        .notifications()
        .list_notifications(authed_request(
            token,
            ListNotificationsRequest {
                page_size: 50,
                page_token: String::new(),
                organisation: Some(org.to_string()),
                project: Some(project.to_string()),
            },
        ))
        .await
        .expect("list notifications")
        .into_inner();

    resp.notifications.into_iter().map(|n| n.title).collect()
}

/// One project, three releases: one each by two people who both have accounts,
/// and one by a bot. Everybody's feed is asserted against all three, because
/// the bug was not "the wrong person is missing a notification" — it was
/// everybody getting everybody's.
#[tokio::test(flavor = "multi_thread")]
async fn a_personal_feed_carries_only_the_releases_that_person_owns() {
    let fixture = fixture().await.unwrap();

    let author = register(&fixture).await;
    let bystander = register(&fixture).await;

    let org = format!("org-{}", uuid::Uuid::now_v7());
    let project = format!("proj-{}", uuid::Uuid::now_v7());

    seed(
        &fixture.db,
        &org,
        &project,
        "authors-own-release",
        Some(author.user_id),
    )
    .await;
    seed(
        &fixture.db,
        &org,
        &project,
        "bystanders-own-release",
        Some(bystander.user_id),
    )
    .await;
    // The screenshot's release: a renovate[bot] PR, which resolves to no forest
    // user at all. It belongs to nobody, so nobody is told personally.
    seed(&fixture.db, &org, &project, "bot-authored-release", None).await;

    let mine = titles_for(&fixture, &author.token, &org, &project).await;
    assert_eq!(
        mine,
        vec!["authors-own-release"],
        "an author should see their own release and nothing else",
    );

    let theirs = titles_for(&fixture, &bystander.token, &org, &project).await;
    assert_eq!(
        theirs,
        vec!["bystanders-own-release"],
        "a non-owner must not receive a personal notification for somebody \
         else's release — the channel already showed it to them",
    );
}

/// The other half of the same guarantee: the fleet feed is untouched. forage's
/// listener reads it under the service-account key and it is what posts the
/// shared release-channel message, so a release with no owner still has to
/// arrive here.
#[tokio::test(flavor = "multi_thread")]
async fn the_service_account_feed_still_carries_every_release() {
    let fixture = fixture().await.unwrap();
    // Same database, a server that has the service-account key configured.
    let service = restricted_fixture().await.unwrap();

    let author = register(&fixture).await;

    let org = format!("org-{}", uuid::Uuid::now_v7());
    let project = format!("proj-{}", uuid::Uuid::now_v7());

    seed(
        &fixture.db,
        &org,
        &project,
        "authors-own-release",
        Some(author.user_id),
    )
    .await;
    seed(&fixture.db, &org, &project, "bot-authored-release", None).await;

    let mut fleet = titles_for(
        &service,
        RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
        &org,
        &project,
    )
    .await;
    fleet.sort();

    assert_eq!(
        fleet,
        vec!["authors-own-release", "bot-authored-release"],
        "the channel feed must see every release, owned or not",
    );
}

/// Preferences layer on top of the owner gate; they do not open it. A person
/// who has never muted anything still gets nothing for a release they do not
/// own, and muting a type they *do* own removes it as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_muted_type_is_still_muted_for_the_owner() {
    let fixture = fixture().await.unwrap();

    let author = register(&fixture).await;

    let org = format!("org-{}", uuid::Uuid::now_v7());
    let project = format!("proj-{}", uuid::Uuid::now_v7());

    seed(
        &fixture.db,
        &org,
        &project,
        "authors-own-release",
        Some(author.user_id),
    )
    .await;

    assert_eq!(
        titles_for(&fixture, &author.token, &org, &project).await,
        vec!["authors-own-release"],
        "the owner sees it before muting",
    );

    fixture
        .notifications()
        .set_notification_preference(authed_request(
            &author.token,
            SetNotificationPreferenceRequest {
                notification_type: NotificationType::ReleaseSucceeded.into(),
                channel: NotificationChannel::Cli.into(),
                enabled: false,
            },
        ))
        .await
        .expect("set preference");

    assert!(
        titles_for(&fixture, &author.token, &org, &project)
            .await
            .is_empty(),
        "a muted type stays muted for the owner too",
    );
}
