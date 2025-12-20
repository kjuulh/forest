use crate::{State, cli::admin::client::GrpcClientState};

#[derive(clap::Parser)]
pub struct AdminCommand {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Status,
}

impl AdminCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let client = state.grpc_client();

        match self.command {
            Command::Status => {
                println!("Testing forest server");
                client
                    .status_get()
                    .await
                    .inspect_err(|e| println!("The forest server is not available\n\n\t{e:#}"))?;

                println!("The forest server is available");
            }
        }

        Ok(())
    }
}

pub mod client {
    use forest_grpc_interface::{GetStatusRequest, status_service_client::StatusServiceClient};
    use tokio::sync::OnceCell;
    use tonic::transport::Channel;

    use crate::State;

    pub struct GrpcClient {
        host: String,
        status_client: OnceCell<StatusServiceClient<Channel>>,
    }

    impl GrpcClient {
        pub async fn status_get(&self) -> anyhow::Result<()> {
            let mut client = self.status_client().await?;

            let resp = client.status(GetStatusRequest {}).await?;
            let _inner = resp.into_inner();

            Ok(())
        }

        async fn status_client(&self) -> anyhow::Result<StatusServiceClient<Channel>> {
            let client = self
                .status_client
                .get_or_try_init(move || async move {
                    let channel = Channel::from_shared(self.host.clone())?.connect().await?;
                    let client = StatusServiceClient::new(channel);

                    Ok::<_, anyhow::Error>(client)
                })
                .await?;

            Ok(client.clone())
        }
    }

    pub trait GrpcClientState {
        fn grpc_client(&self) -> GrpcClient;
    }

    impl GrpcClientState for State {
        fn grpc_client(&self) -> GrpcClient {
            GrpcClient {
                host: self
                    .config
                    .external_host
                    .clone()
                    .expect("to get able to get external_host"),
                status_client: OnceCell::new(),
            }
        }
    }
}
