//! Terraform state ids per destination, without moving any existing state and
//! without leaving an older server blind (understory-io/forest#285).

use forest_grpc_interface::*;
use forest_server::destinations::terraformv1::{TerraformStateStore, TerraformStateStoreState};
use tonic::metadata::MetadataValue;

use super::fixtures::{Fixture, fixture};

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", val);
    req
}

/// An org and an environment with real terraform destinations in it, so the
/// prior-release check has rows to join against.
struct World {
    org: String,
    env: String,
    token: String,
}

async fn world(f: &Fixture) -> World {
    let token = f
        .users()
        .register(RegisterRequest {
            username: format!("user-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .tokens
        .unwrap()
        .access_token;
    let org = format!("org-{}", uuid::Uuid::now_v7());
    f.organisations()
        .create_organisation(authed(
            &token,
            CreateOrganisationRequest { name: org.clone() },
        ))
        .await
        .unwrap();
    let env = format!("env-{}", uuid::Uuid::now_v7());
    f.environments()
        .create_environment(authed(
            &token,
            CreateEnvironmentRequest {
                organisation: org.clone(),
                name: env.clone(),
                description: None,
                sort_order: 0,
            },
        ))
        .await
        .unwrap();
    World { org, env, token }
}

impl World {
    async fn destination(&self, f: &Fixture, name: &str) -> forest_models::Destination {
        f.destinations()
            .create_destination(authed(
                &self.token,
                CreateDestinationRequest {
                    organisation: self.org.clone(),
                    name: name.to_string(),
                    environment: self.env.clone(),
                    metadata: Default::default(),
                    r#type: Some(DestinationType {
                        organisation: "forest".into(),
                        name: "terraform".into(),
                        version: 1,
                        description: String::new(),
                        fields: vec![],
                    }),
                    sensitive_keys: vec![],
                },
            ))
            .await
            .unwrap();
        forest_models::Destination {
            organisation: self.org.clone(),
            name: name.into(),
            environment: self.env.clone(),
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
}

/// A release of `project` to `destination` that finished an hour ago — what a
/// destination sharing a legacy state before #285 has in its history.
async fn completed_release_an_hour_ago(f: &Fixture, org: &str, destination: &str, project: &str) {
    let destination_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM destinations WHERE organisation = $1 AND name = $2")
            .bind(org)
            .bind(destination)
            .fetch_one(&f.db)
            .await
            .unwrap();
    let project: uuid::Uuid = project.parse().unwrap();
    let intent: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO release_intents (artifact, annotation_id, project_id)
         VALUES (gen_random_uuid(), gen_random_uuid(), $1) RETURNING id",
    )
    .bind(project)
    .fetch_one(&f.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO release_states
            (release_intent_id, project_id, destination_id, artifact_id, status,
             queued_at, started_at, completed_at)
         VALUES ($1, $2, $3, gen_random_uuid(), 'SUCCEEDED',
                 now() - interval '2 hours', now() - interval '2 hours', now() - interval '1 hour')",
    )
    .bind(intent)
    .bind(project)
    .bind(destination_id)
    .execute(&f.db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_with_one_destination_keeps_the_legacy_id() {
    // Every project today, and every new one with one terraform destination
    // per environment: `<env>.<project>`, which an older server also reads.
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    let dest = w.destination(&f, "tf-only").await;
    let project = uuid::Uuid::new_v4().to_string();
    let store = f.state.terraform_state_store();

    for _ in 0..2 {
        assert_eq!(
            store.resolve_state_id(&dest, &project).await.unwrap(),
            TerraformStateStore::legacy_state_id(&w.env, &project)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_existing_state_keeps_its_id() {
    // tailscale, hetzner, screen, fungus: applied under `<env>.<project>`
    // before this change, with no binding yet.
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    let dest = w.destination(&f, "hetzner-dev").await;
    let project = uuid::Uuid::new_v4().to_string();
    let legacy = TerraformStateStore::legacy_state_id(&w.env, &project);
    sqlx::query("INSERT INTO terraform_states (state_id, content) VALUES ($1, '{}')")
        .bind(&legacy)
        .execute(&f.db)
        .await
        .unwrap();

    assert_eq!(
        f.state
            .terraform_state_store()
            .resolve_state_id(&dest, &project)
            .await
            .unwrap(),
        legacy
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_destination_gets_its_own_state_and_the_first_keeps_legacy() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    let first = w.destination(&f, "dev/eu-west-1/first").await;
    let second = w.destination(&f, "dev/eu-west-1/second").await;
    let project = uuid::Uuid::new_v4().to_string();
    let store = f.state.terraform_state_store();

    let a = store.resolve_state_id(&first, &project).await.unwrap();
    let b = store.resolve_state_id(&second, &project).await.unwrap();

    assert_eq!(a, TerraformStateStore::legacy_state_id(&w.env, &project));
    assert_eq!(
        b,
        TerraformStateStore::destination_state_id(&w.env, &project, "dev/eu-west-1/second")
    );

    // Stable, whichever order they run in from now on.
    assert_eq!(store.resolve_state_id(&second, &project).await.unwrap(), b);
    assert_eq!(store.resolve_state_id(&first, &project).await.unwrap(), a);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_destination_that_shared_the_legacy_state_is_refused_a_fresh_one() {
    // Before #285 both destinations wrote to `<env>.<project>`. The first to
    // run after the change claims it; the other must not get an empty state
    // that would plan to re-create everything it already made.
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    let first = w.destination(&f, "dev/eu-west-1/first").await;
    let second = w.destination(&f, "dev/eu-west-1/second").await;
    let project = uuid::Uuid::new_v4().to_string();
    completed_release_an_hour_ago(&f, &w.org, "dev/eu-west-1/second", &project).await;
    let store = f.state.terraform_state_store();

    store.resolve_state_id(&first, &project).await.unwrap();

    let err = store
        .resolve_state_id(&second, &project)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("while its terraform state was shared"),
        "{err}"
    );
    assert!(err.contains("dev/eu-west-1/first"), "{err}");

    // The owner is unaffected.
    assert_eq!(
        store.resolve_state_id(&first, &project).await.unwrap(),
        TerraformStateStore::legacy_state_id(&w.env, &project)
    );
}
