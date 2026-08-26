//! A project's release history must not be cut off at an arbitrary depth.
//!
//! forage rebuilds each release's destinations from these intents' steps
//! (DATA-660). A release past the cap has none, falls back to the
//! current-state-per-destination view, matches nothing, and renders as an
//! undeployed commit with a blank swim lane — so the cap silently put a floor
//! under how far the Releases timeline could go (DATA-662).
//!
//! Project-scoped lookups are therefore uncapped; org-wide ones, which span
//! every project and are a recent-activity feed, stay capped.

use forest_grpc_interface::GetReleaseIntentStatesRequest;

use crate::accepttest::fixtures::{Fixture, fixture};

/// Local copy: the identical helper in `authz_flow` is module-private, and the
/// other flow modules each keep their own rather than widening it.
fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: tonic::metadata::MetadataValue<_> =
        format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

/// More intents than the org-wide cap, so "all of them" and "the cap" are
/// distinguishable in the assertions below.
const SEEDED: usize = 60;
const ORG_CAP: usize = 50;

async fn register_user(fixture: &Fixture) -> String {
    let mut users = fixture.users();
    let username = format!("rih-{}", uuid::Uuid::now_v7());
    let email = format!("{}@test.com", uuid::Uuid::now_v7());
    let resp = users
        .register(forest_grpc_interface::RegisterRequest {
            username,
            email,
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user")
        .into_inner();
    resp.tokens.expect("tokens").access_token
}

async fn create_org(fixture: &Fixture, token: &str) -> String {
    let name = format!("org-{}", uuid::Uuid::now_v7());
    fixture
        .organisations()
        .create_organisation(authed_request(
            token,
            forest_grpc_interface::CreateOrganisationRequest { name: name.clone() },
        ))
        .await
        .expect("create organisation");
    name
}

/// Seed `SEEDED` completed intents for one project, straight into the tables.
/// Going through the release API would mean 60 full release round-trips to
/// exercise a read-path limit.
async fn seed_intents(db: &sqlx::PgPool, org: &str) -> String {
    let project = format!("proj-{}", uuid::Uuid::now_v7());
    let project_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO projects (organisation, project) VALUES ($1, $2) RETURNING id",
    )
    .bind(org)
    .bind(&project)
    .fetch_one(db)
    .await
    .expect("insert project");

    for i in 0..SEEDED {
        sqlx::query(
            "INSERT INTO release_intents (artifact, annotation_id, project_id, status, created)
             VALUES ($1, $2, $3, 'COMPLETED', now() - make_interval(secs => $4::double precision))",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(uuid::Uuid::now_v7())
        .bind(project_id)
        // Descending age, so `ORDER BY created DESC` has a stable order and a
        // cap would drop the oldest — the ones a deep page needs.
        .bind((SEEDED - i) as f64)
        .execute(db)
        .await
        .expect("insert release intent");
    }

    project
}

#[tokio::test(flavor = "multi_thread")]
async fn project_scoped_intents_return_the_whole_history() {
    let fixture = fixture().await.unwrap();
    let token = register_user(&fixture).await;
    let org = create_org(&fixture, &token).await;
    let project = seed_intents(&fixture.db, &org).await;

    let resp = fixture
        .releases()
        .get_release_intent_states(authed_request(
            &token,
            GetReleaseIntentStatesRequest {
                organisation: org.clone(),
                project: Some(project),
                include_completed: true,
            },
        ))
        .await
        .expect("get release intent states")
        .into_inner();

    assert_eq!(
        resp.release_intents.len(),
        SEEDED,
        "a project-scoped lookup must return every intent, not the {ORG_CAP} most recent — \
         anything it drops renders as an undeployed commit on the timeline",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn org_wide_intents_stay_capped() {
    let fixture = fixture().await.unwrap();
    let token = register_user(&fixture).await;
    let org = create_org(&fixture, &token).await;
    seed_intents(&fixture.db, &org).await;

    let resp = fixture
        .releases()
        .get_release_intent_states(authed_request(
            &token,
            GetReleaseIntentStatesRequest {
                organisation: org,
                project: None,
                include_completed: true,
            },
        ))
        .await
        .expect("get release intent states")
        .into_inner();

    assert_eq!(
        resp.release_intents.len(),
        ORG_CAP,
        "the org-wide feed spans every project and stays a recency window",
    );
}
