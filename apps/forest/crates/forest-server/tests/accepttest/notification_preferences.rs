use forest_grpc_interface::*;
use forest_server::services::notification_registry::{NotificationRegistryState, ReleaseContext};

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
            username: format!("prefs-{}", uuid::Uuid::now_v7()),
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

async fn set(fixture: &Fixture, person: &Person, ntype: NotificationType, enabled: bool) {
    fixture
        .notifications()
        .set_notification_preference(authed_request(
            &person.token,
            SetNotificationPreferenceRequest {
                notification_type: ntype.into(),
                channel: NotificationChannel::Slack.into(),
                enabled,
            },
        ))
        .await
        .expect("set preference");
}

async fn own_prefs(fixture: &Fixture, person: &Person) -> Vec<NotificationPreference> {
    fixture
        .notifications()
        .get_notification_preferences(authed_request(
            &person.token,
            GetNotificationPreferencesRequest {},
        ))
        .await
        .expect("get own preferences")
        .into_inner()
        .preferences
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_account_reads_a_users_explicit_preferences() {
    let fixture = fixture().await.unwrap();
    let service = restricted_fixture().await.unwrap();
    let person = register(&fixture).await;

    set(&fixture, &person, NotificationType::ReleaseFailed, false).await;

    let prefs = service
        .notifications()
        .get_user_notification_preferences(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            GetUserNotificationPreferencesRequest {
                user_id: person.user_id.to_string(),
            },
        ))
        .await
        .expect("service account reads preferences")
        .into_inner()
        .preferences;

    assert_eq!(
        prefs,
        vec![NotificationPreference {
            notification_type: NotificationType::ReleaseFailed.into(),
            channel: NotificationChannel::Slack.into(),
            enabled: false,
        }],
        "only the explicit row comes back; absent rows stay absent",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_cannot_read_somebody_elses_preferences() {
    let fixture = fixture().await.unwrap();
    let reader = register(&fixture).await;
    let target = register(&fixture).await;

    for user_id in [target.user_id, reader.user_id] {
        let status = fixture
            .notifications()
            .get_user_notification_preferences(authed_request(
                &reader.token,
                GetUserNotificationPreferencesRequest {
                    user_id: user_id.to_string(),
                },
            ))
            .await
            .expect_err("a user token must be refused, even for their own id");

        assert!(
            matches!(
                status.code(),
                tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
            ),
            "unexpected status {status:?}",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_account_must_name_a_valid_user_id() {
    let service = restricted_fixture().await.unwrap();

    let status = service
        .notifications()
        .get_user_notification_preferences(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            GetUserNotificationPreferencesRequest {
                user_id: "not-a-uuid".into(),
            },
        ))
        .await
        .expect_err("invalid id");

    assert_eq!(status.code(), tonic::Code::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_removes_only_the_callers_row_and_is_idempotent() {
    let fixture = fixture().await.unwrap();
    let me = register(&fixture).await;
    let other = register(&fixture).await;

    set(&fixture, &me, NotificationType::ReleaseFailed, false).await;
    set(&fixture, &me, NotificationType::ReleaseStarted, true).await;
    set(&fixture, &other, NotificationType::ReleaseFailed, false).await;

    for _ in 0..2 {
        fixture
            .notifications()
            .reset_notification_preference(authed_request(
                &me.token,
                ResetNotificationPreferenceRequest {
                    notification_type: NotificationType::ReleaseFailed.into(),
                    channel: NotificationChannel::Slack.into(),
                },
            ))
            .await
            .expect("reset is idempotent");
    }

    let mine = own_prefs(&fixture, &me).await;
    assert_eq!(
        mine,
        vec![NotificationPreference {
            notification_type: NotificationType::ReleaseStarted.into(),
            channel: NotificationChannel::Slack.into(),
            enabled: true,
        }],
        "only the reset row is gone",
    );

    assert_eq!(
        own_prefs(&fixture, &other).await.len(),
        1,
        "another user's identical row is untouched",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_attribution_travels_with_the_notification() {
    let fixture = fixture().await.unwrap();
    let author = register(&fixture).await;

    let org = format!("org-{}", uuid::Uuid::now_v7());
    let project = format!("proj-{}", uuid::Uuid::now_v7());

    fixture
        .state
        .notification_registry()
        .create_notification(
            "RELEASE_SUCCEEDED",
            "attributed-release",
            "Release succeeded",
            &org,
            &project,
            &ReleaseContext {
                slug: Some(format!("rel-{}", uuid::Uuid::now_v7())),
                source_user_id: Some(author.user_id.to_string()),
                source_attribution: Some("actor".into()),
                ..Default::default()
            },
        )
        .await
        .expect("seed notification");

    let notifications = fixture
        .notifications()
        .list_notifications(authed_request(
            &author.token,
            ListNotificationsRequest {
                page_size: 10,
                page_token: String::new(),
                organisation: Some(org),
                project: Some(project),
            },
        ))
        .await
        .expect("list notifications")
        .into_inner()
        .notifications;

    assert_eq!(notifications.len(), 1);
    let context = notifications[0].release_context.as_ref().expect("context");
    assert_eq!(context.source_attribution, "actor");
    assert_eq!(context.source_user_id, author.user_id.to_string());
}
