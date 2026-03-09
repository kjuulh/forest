use std::net::SocketAddr;
use std::sync::{LazyLock, OnceLock};

use forest_grpc_interface::artifact_service_client::ArtifactServiceClient;
use forest_grpc_interface::destination_service_client::DestinationServiceClient;
use forest_grpc_interface::environment_service_client::EnvironmentServiceClient;
use forest_grpc_interface::organisation_service_client::OrganisationServiceClient;
use forest_grpc_interface::registry_service_client::RegistryServiceClient;
use forest_grpc_interface::release_service_client::ReleaseServiceClient;
use forest_grpc_interface::users_service_client::UsersServiceClient;
use tonic::transport::Channel;

#[derive(Clone)]
pub struct Fixture {
    pub channel: Channel,
    pub db: sqlx::PgPool,
}

impl Fixture {
    pub fn users(&self) -> UsersServiceClient<Channel> {
        UsersServiceClient::new(self.channel.clone())
    }

    pub fn artifacts(&self) -> ArtifactServiceClient<Channel> {
        ArtifactServiceClient::new(self.channel.clone())
    }

    pub fn releases(&self) -> ReleaseServiceClient<Channel> {
        ReleaseServiceClient::new(self.channel.clone())
    }

    pub fn organisations(&self) -> OrganisationServiceClient<Channel> {
        OrganisationServiceClient::new(self.channel.clone())
    }

    pub fn destinations(&self) -> DestinationServiceClient<Channel> {
        DestinationServiceClient::new(self.channel.clone())
    }

    pub fn environments(&self) -> EnvironmentServiceClient<Channel> {
        EnvironmentServiceClient::new(self.channel.clone())
    }

    pub fn registry(&self) -> RegistryServiceClient<Channel> {
        RegistryServiceClient::new(self.channel.clone())
    }
}

/// Dedicated runtime that outlives all tests, so spawned server/scheduler tasks
/// are never dropped when an individual `#[tokio::test]` runtime shuts down.
static FIXTURE_RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build fixture runtime")
});

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

pub async fn fixture() -> anyhow::Result<Fixture> {
    let fixture = FIXTURE.get_or_init(|| {
        // Use block_in_place to allow blocking inside a multi-thread tokio runtime,
        // then block_on the fixture runtime so server tasks are spawned there.
        tokio::task::block_in_place(|| FIXTURE_RUNTIME.block_on(async {
            dotenvy::dotenv().ok();

            // Initialize tracing for test output
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .with_test_writer()
                .try_init();

            let config = forest_server::Config {
                external_host: "http://localhost:0".into(),
                terraform_external_host: "http://localhost:0".into(),
                password_secret_key: "test-password-secret-key-32chars".into(),
                access_token_secret_key: b"test-access-token-secret-key-32b".to_vec(),
                refresh_token_secret_key: b"test-refresh-token-secret-key32b".to_vec(),
                service_account_token_hash: None,
            };

            let state = forest_server::State::new(config)
                .await
                .expect("failed to create state (is DATABASE_URL set?)");

            let db = state.db.clone();

            // Clean test data
            clean_database(&db).await;

            // Bind to random port
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind to random port");
            let addr = listener.local_addr().expect("get local addr");
            drop(listener);

            let runner_manager = forest_server::runner_manager::RunnerManager::new();

            // Start gRPC server on the fixture runtime so it outlives individual tests
            let cancel = tokio_util::sync::CancellationToken::new();
            {
                let state = state.clone();
                let runner_manager = runner_manager.clone();
                let cancel = cancel.clone();
                FIXTURE_RUNTIME.spawn(async move {
                    let grpc = forest_server::grpc::GrpcServer {
                        host: addr,
                        state: state.clone(),
                        runner_manager: runner_manager.clone(),
                    };
                    grpc.serve(cancel).await.ok();
                });
            }

            // Start scheduler as a Component
            {
                let state = state.clone();
                let runner_manager = runner_manager.clone();
                let cancel = cancel.clone();
                FIXTURE_RUNTIME.spawn(async move {
                    use notmad::Component;
                    let sched =
                        forest_server::scheduler::Scheduler::new(&state, runner_manager, false);
                    sched.run(cancel).await.ok();
                });
            }

            // Wait for server to be ready
            probe_grpc(addr).await;

            let channel = Channel::from_shared(format!("http://{}", addr))
                .expect("valid uri")
                .connect()
                .await
                .expect("connect to grpc server");

            Fixture { channel, db }
        }))
    });



    Ok(fixture.clone())
}

async fn clean_database(db: &sqlx::PgPool) {
    let tables = [
        "release_events",
        "release_logs",
        "release_states",
        "release_tokens",
        "release_intents",
        "annotations",
        "artifact_files",
        "artifacts",
        "artifact_staging",
        "blob_storage",
        "component_files",
        "component_staging",
        "components",
        "destinations",
        "environments",
        "notifications",
        "user_sessions",
        "user_emails",
        "personal_access_tokens",
        "user_mfa",
        "user_oauth_connections",
        "users",
        "organisation_members",
        "organisations",
        "projects",
        "es_events",
        "es_streams",
    ];
    for table in tables {
        let query = format!("DELETE FROM {}", table);
        sqlx::query(&query).execute(db).await.ok();
    }
}

async fn probe_grpc(addr: SocketAddr) {
    for i in 0..40 {
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            Channel::from_shared(format!("http://{}", addr))
                .unwrap()
                .connect(),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                eprintln!("grpc server ready at {addr}");
                return;
            }
            _ => {
                eprintln!("waiting for grpc server... attempt {}", i + 1);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    panic!("failed to connect to grpc server at {addr}");
}
