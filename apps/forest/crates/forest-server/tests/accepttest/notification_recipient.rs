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

#[tokio::test(flavor = "multi_thread")]
async fn the_service_account_feed_still_carries_every_release() {
    let fixture = fixture().await.unwrap();
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
