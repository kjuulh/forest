use std::{collections::HashMap, pin::Pin, time::Duration};

use forest_grpc_interface::provider::{
    DescribeRequest, DescribeResponse, ExecuteEvent, ExecuteRequest, Outcome,
    destination_provider_server::{DestinationProvider, DestinationProviderServer},
    execute_event::Event,
};
use forest_grpc_interface::{CreateDestinationRequest, DestinationType};
use forest_server::release_drain::{DrainOutcome, INTERRUPTED, ReleaseDrain};
use forest_server::runner_manager::RunnerManager;
use forest_server::scheduler::Scheduler;
use futures::Stream;
use uuid::Uuid;

use crate::accepttest::fixtures::{Fixture, GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

const BEHAVIOUR: &str = "drain_test_behaviour";

struct StubProvider;

#[tonic::async_trait]
impl DestinationProvider for StubProvider {
    async fn describe(
        &self,
        _request: tonic::Request<DescribeRequest>,
    ) -> Result<tonic::Response<DescribeResponse>, tonic::Status> {
        Ok(tonic::Response::new(DescribeResponse {
            description: "drain test provider".into(),
            fields: vec![],
            supports_plan: false,
            type_name: String::new(),
        }))
    }

    type ExecuteStream =
        Pin<Box<dyn Stream<Item = Result<ExecuteEvent, tonic::Status>> + Send + 'static>>;

    async fn execute(
        &self,
        request: tonic::Request<ExecuteRequest>,
    ) -> Result<tonic::Response<Self::ExecuteStream>, tonic::Status> {
        let behaviour = request
            .into_inner()
            .destination
            .and_then(|d| d.metadata.get(BEHAVIOUR).cloned())
            .unwrap_or_default();

        let stream = async_stream(behaviour);
        Ok(tonic::Response::new(Box::pin(stream)))
    }
}

fn async_stream(behaviour: String) -> impl Stream<Item = Result<ExecuteEvent, tonic::Status>> {
    futures::stream::once(async move {
        match behaviour.as_str() {
            "hang" => std::future::pending::<()>().await,
            _ => tokio::time::sleep(Duration::from_millis(1500)).await,
        }
        Ok(ExecuteEvent {
            event: Some(Event::Outcome(Outcome {
                success: true,
                error_message: String::new(),
                plan_output: String::new(),
                ..Default::default()
            })),
        })
    })
}

async fn start_stub_provider() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the stub provider");
    let addr = listener.local_addr().expect("stub provider address");
    drop(listener);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(DestinationProviderServer::new(StubProvider))
            .serve(addr)
            .await
            .ok();
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    unsafe {
        std::env::set_var(
            forest_server::destinations::genericv1::ALLOWED_HOSTS_ENV,
            "127.0.0.1",
        );
    }
    format!("http://{addr}")
}

struct Hold {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
    xid: String,
}

impl Hold {
    async fn take(fixture: &Fixture, release_id: Uuid) -> anyhow::Result<Self> {
        let mut tx = fixture.db.begin().await?;
        sqlx::query("SELECT 1 FROM release_states WHERE release_id = $1 FOR UPDATE")
            .bind(release_id)
            .execute(&mut *tx)
            .await?;
        let xid: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
            .fetch_one(&mut *tx)
            .await?;
        Ok(Self { tx, xid })
    }

    async fn wait_for_a_claim(&self, fixture: &Fixture) -> anyhow::Result<()> {
        for _ in 0..200 {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks
                 WHERE locktype = 'transactionid' AND transactionid::text = $1 AND NOT granted",
            )
            .bind(&self.xid)
            .fetch_one(&fixture.db)
            .await?;
            if waiting > 0 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        anyhow::bail!("no scheduler tried to claim the held release")
    }
}

struct Queued {
    fixture: Fixture,
    project_id: Uuid,
    destination_id: Uuid,
    release_id: Uuid,
    hold: Option<Hold>,
}

impl Queued {
    async fn new(behaviour: &str) -> anyhow::Result<Self> {
        let provider_url = start_stub_provider().await;
        let (given, _, _) = testcase::<ReleaseFlowData>().await?;
        let suffix = Uuid::now_v7();
        let organisation = format!("test-org-{suffix}");
        let environment = format!("drain-env-{suffix}");
        let given = given
            .a_registered_user()
            .await
            .an_organisation(&organisation)
            .await
            .an_environment(&environment)
            .await
            .a_destination_of_type("drain-noop", &environment, "forest/noop@1")
            .await
            .an_uploaded_artifact()
            .await
            .an_annotated_release()
            .await;
        let fixture = given.fixture().clone();
        let token = given.data().auth_token.clone();
        let artifact_id: Uuid = given.data().artifact_id.parse()?;

        let mut request = tonic::Request::new(CreateDestinationRequest {
            organisation: organisation.clone(),
            name: "drain-generic".into(),
            environment: environment.clone(),
            metadata: HashMap::from([
                ("provider_url".to_string(), provider_url),
                (BEHAVIOUR.to_string(), behaviour.to_string()),
            ]),
            r#type: Some(DestinationType {
                organisation: "forest".into(),
                name: "generic".into(),
                version: 1,
                description: String::new(),
                fields: vec![],
            }),
            sensitive_keys: vec![],
        });
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse()?);
        fixture
            .destinations()
            .create_destination(request)
            .await
            .expect("create the generic destination");

        let (annotation_id, project_id): (Uuid, Uuid) =
            sqlx::query_as("SELECT id, project_id FROM annotations WHERE artifact_id = $1")
                .bind(artifact_id)
                .fetch_one(&fixture.db)
                .await?;
        let destination_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM destinations WHERE organisation = $1 AND name = 'drain-generic'",
        )
        .bind(&organisation)
        .fetch_one(&fixture.db)
        .await?;

        let release_id = insert_queued_release(
            &fixture,
            project_id,
            destination_id,
            annotation_id,
            artifact_id,
        )
        .await?;
        let hold = Hold::take(&fixture, release_id).await?;

        Ok(Self {
            fixture,
            project_id,
            destination_id,
            release_id,
            hold: Some(hold),
        })
    }

    async fn let_the_claim_through(&mut self) -> anyhow::Result<()> {
        let hold = self.hold.take().expect("the release is held until claimed");
        hold.wait_for_a_claim(&self.fixture).await?;
        hold.tx.commit().await?;
        Ok(())
    }

    async fn cancel_while_held(mut self) -> anyhow::Result<()> {
        let mut hold = self.hold.take().expect("the release is still held");
        sqlx::query(
            "UPDATE release_states
                SET status = 'CANCELLED', error_message = 'acceptance test cleanup',
                    completed_at = now(), updated_at = now()
              WHERE release_id = $1",
        )
        .bind(self.release_id)
        .execute(&mut *hold.tx)
        .await?;
        hold.tx.commit().await?;
        Ok(())
    }

    fn scheduler(&self, drain: ReleaseDrain) -> Scheduler {
        Scheduler::new(&self.fixture.state, RunnerManager::new(), false, drain)
    }

    async fn status(&self) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, error_message FROM release_states WHERE release_id = $1")
            .bind(self.release_id)
            .fetch_one(&self.fixture.db)
            .await
            .expect("release row")
    }

    async fn assigned_events(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM release_events
             WHERE release_id = $1 AND event_type = 'release.assigned'",
        )
        .bind(self.release_id)
        .fetch_one(&self.fixture.db)
        .await
        .expect("event count")
    }

    async fn wait_until_running(&self) {
        for _ in 0..100 {
            if self.status().await.0 == "RUNNING" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("release never started: {:?}", self.status().await);
    }

    async fn lock_as_this_release(&self, lock_id: &str) -> anyhow::Result<String> {
        let state_id = format!("test.drain.{}", Uuid::new_v4());
        let replica = forest_server::replicas::replica_name();
        sqlx::query(
            "INSERT INTO terraform_state_locks
                 (state_id, lock_id, operation, principal, release_id, replica)
             VALUES ($1, $2, 'OperationTypeApply', $3, $4, $5)",
        )
        .bind(&state_id)
        .bind(lock_id)
        .bind(format!("release:{}@{replica}:test/drain", self.release_id))
        .bind(self.release_id)
        .bind(replica)
        .execute(&self.fixture.db)
        .await?;
        Ok(state_id)
    }

    async fn lock_of(&self, state_id: &str) -> anyhow::Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT lock_id FROM terraform_state_locks WHERE state_id = $1")
                .bind(state_id)
                .fetch_optional(&self.fixture.db)
                .await?,
        )
    }

    async fn cleanup(&self) {
        let _ = sqlx::query(
            "UPDATE release_states
                SET status = 'CANCELLED', error_message = 'acceptance test cleanup',
                    completed_at = now(), updated_at = now()
              WHERE project_id = $1 AND destination_id = $2
                AND status IN ('QUEUED', 'ASSIGNED', 'RUNNING')",
        )
        .bind(self.project_id)
        .bind(self.destination_id)
        .execute(&self.fixture.db)
        .await;
    }
}

async fn insert_queued_release(
    fixture: &Fixture,
    project_id: Uuid,
    destination_id: Uuid,
    annotation_id: Uuid,
    artifact_id: Uuid,
) -> anyhow::Result<Uuid> {
    let intent_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO release_intents (id, artifact, annotation_id, project_id, status)
         VALUES ($1, $2, $3, $4, 'ACTIVE')",
    )
    .bind(intent_id)
    .bind(artifact_id)
    .bind(annotation_id)
    .bind(project_id)
    .execute(&fixture.db)
    .await?;

    let release_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO release_states (
             release_id, release_intent_id, project_id, destination_id, artifact_id, status
         ) VALUES ($1, $2, $3, $4, $5, 'QUEUED')",
    )
    .bind(release_id)
    .bind(intent_id)
    .bind(project_id)
    .bind(destination_id)
    .bind(artifact_id)
    .execute(&fixture.db)
    .await?;

    Ok(release_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_release_running_at_shutdown_finishes_within_the_drain_window() -> anyhow::Result<()> {
    let mut queued = Queued::new("finish").await?;
    let scheduler = queued.scheduler(ReleaseDrain::with_graces(
        Duration::from_secs(20),
        Duration::from_secs(1),
        Duration::from_secs(1),
    ));

    scheduler.dispatch(queued.release_id);
    queued.let_the_claim_through().await?;
    queued.wait_until_running().await;

    assert_eq!(scheduler.drain().drain().await, DrainOutcome::Finished);
    assert_eq!(
        queued.status().await,
        ("SUCCEEDED".to_string(), None),
        "the drain waited for the in-flight release instead of cutting it off"
    );

    queued.cleanup().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_release_still_running_at_the_deadline_fails_at_once_as_interrupted() -> anyhow::Result<()>
{
    let mut queued = Queued::new("hang").await?;
    let scheduler = queued.scheduler(ReleaseDrain::with_graces(
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(10),
    ));

    scheduler.dispatch(queued.release_id);
    queued.let_the_claim_through().await?;
    queued.wait_until_running().await;

    let state_id = queued.lock_as_this_release("interrupted-run").await?;

    assert_eq!(scheduler.drain().drain().await, DrainOutcome::HandedOver);
    let (status, error) = queued.status().await;
    assert_eq!(
        status, "FAILED",
        "the drain hands the release over itself instead of leaving it to the reaper"
    );
    let error = error.unwrap_or_default();
    assert!(error.contains(INTERRUPTED), "got {error:?}");

    assert_eq!(
        queued.lock_of(&state_id).await?,
        None,
        "the interrupted release's terraform state lock is released with it"
    );

    queued.cleanup().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_release_gives_up_its_lock_even_when_its_failure_cannot_be_recorded()
-> anyhow::Result<()> {
    let mut queued = Queued::new("hang").await?;
    let scheduler = queued.scheduler(ReleaseDrain::with_graces(
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(10),
    ));

    scheduler.dispatch(queued.release_id);
    queued.let_the_claim_through().await?;
    queued.wait_until_running().await;

    let state_id = queued.lock_as_this_release("unrecorded-run").await?;
    sqlx::query(
        "UPDATE release_states
            SET status = 'CANCELLED', error_message = 'cancelled while running',
                completed_at = now(), updated_at = now()
          WHERE release_id = $1",
    )
    .bind(queued.release_id)
    .execute(&queued.fixture.db)
    .await?;

    assert_eq!(scheduler.drain().drain().await, DrainOutcome::HandedOver);
    assert_eq!(
        queued.status().await.0,
        "CANCELLED",
        "recording the failure was refused, so the release keeps its status"
    );
    assert_eq!(
        queued.lock_of(&state_id).await?,
        None,
        "the lock is released although emitting the failure failed"
    );

    queued.cleanup().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draining_scheduler_claims_no_new_release() -> anyhow::Result<()> {
    let queued = Queued::new("finish").await?;
    let scheduler = queued.scheduler(ReleaseDrain::with_graces(
        Duration::from_secs(5),
        Duration::from_secs(1),
        Duration::from_secs(1),
    ));

    assert_eq!(scheduler.drain().drain().await, DrainOutcome::Finished);
    scheduler.dispatch(queued.release_id);
    assert_eq!(scheduler.drain().drain().await, DrainOutcome::Finished);

    assert_eq!(
        queued.status().await.0,
        "QUEUED",
        "a draining replica leaves the release for another replica"
    );
    assert_eq!(queued.assigned_events().await, 0);

    queued.cancel_while_held().await
}
