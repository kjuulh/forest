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
    /// Mock DNS resolver injected at startup so tests can preload TXT
    /// records (`fixture.dns.set_txt(name, value)`) without performing
    /// real network lookups. Used by the org-allowed-domain verification
    /// flow (DATA-252).
    pub dns: std::sync::Arc<forest_server::dns::MockDnsResolver>,
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
static RESTRICTED_FIXTURE: OnceLock<Fixture> = OnceLock::new();

fn base_test_config() -> forest_server::Config {
    forest_server::Config {
        external_host: "http://localhost:0".into(),
        terraform_external_host: "http://localhost:0".into(),
        password_secret_key: "test-password-secret-key-32chars".into(),
        access_token_secret_key: b"test-access-token-secret-key-32b".to_vec(),
        refresh_token_secret_key: b"test-refresh-token-secret-key32b".to_vec(),
        service_account_token_hash: None,
        registration_email_domain_regex: None,
        require_email_verification: false,
        web_app_url: Some("http://forage.test.invalid".into()),
    }
}

fn bring_up(config: forest_server::Config) -> Fixture {
    tokio::task::block_in_place(|| FIXTURE_RUNTIME.block_on(async move {
        dotenvy::dotenv().ok();

        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();

        // Inject a mock DNS resolver — tests must not perform real
        // network DNS lookups, and HickoryResolver::from_system() also
        // requires a usable /etc/resolv.conf which CI may not have.
        let mock_dns = std::sync::Arc::new(forest_server::dns::MockDnsResolver::new());
        let state = forest_server::State::new_with_dns(config, mock_dns.clone())
            .await
            .expect("failed to create state (is DATABASE_URL or TEST_DATABASE_URL set?)");

        let db = state.db.clone();

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

        Fixture { channel, db, dns: mock_dns }
    }))
}

pub async fn fixture() -> anyhow::Result<Fixture> {
    let fixture = FIXTURE.get_or_init(|| bring_up(base_test_config()));
    Ok(fixture.clone())
}

/// Plaintext service-account key configured on `restricted_fixture()`.
/// Tests pass this in an `authorization: Bearer …` header to call
/// service-account-only RPCs (e.g. `ConfirmEmailVerification`).
pub const RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY: &str = "test-service-account-key";

/// Fixture with the registration domain regex set to `@understory\.io$`
/// (and the email-verification flag on so startup validation passes).
/// Shares the underlying DB with the default fixture; tests rely on
/// UUID-suffixed identifiers for isolation.
pub async fn restricted_fixture() -> anyhow::Result<Fixture> {
    let fixture = RESTRICTED_FIXTURE.get_or_init(|| {
        use sha2::Digest;
        let mut config = base_test_config();
        config.registration_email_domain_regex =
            Some(regex::Regex::new(r"@understory\.io$").unwrap());
        config.require_email_verification = true;
        config.service_account_token_hash = Some(
            sha2::Sha256::digest(RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY.as_bytes()).to_vec(),
        );
        bring_up(config)
    });
    Ok(fixture.clone())
}

/// Test helper: flip `user_emails.verified` to true via direct DB
/// access, simulating the side-effect of redeeming a verification token
/// in forage. Acceptance tests use this when the gate
/// `require_email_verification = true` blocks login otherwise.
pub async fn mark_email_verified(
    db: &sqlx::PgPool,
    user_id: uuid::Uuid,
    email: &str,
) -> anyhow::Result<()> {
    sqlx::query!(
        r#"UPDATE user_emails SET verified = true WHERE user_id = $1 AND email = $2"#,
        user_id,
        email,
    )
    .execute(db)
    .await?;
    Ok(())
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
