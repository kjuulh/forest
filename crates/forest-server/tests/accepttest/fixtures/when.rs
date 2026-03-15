use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::release_flow::ReleaseFlowData;

use super::When;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

pub trait WhenReleaseFlow {
    async fn release_is_triggered(self) -> anyhow::Result<When<ReleaseFlowData>>;
    async fn release_reaches_terminal_state(self) -> anyhow::Result<When<ReleaseFlowData>>;
}

impl WhenReleaseFlow for When<ReleaseFlowData> {
    async fn release_is_triggered(self) -> anyhow::Result<Self> {
        let mut release_client = self.fixture().releases();
        let (token, artifact_id, destination) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.artifact_id.clone(),
                data.destination_name.clone(),
            )
        };

        let resp = release_client
            .release(authed_request(
                &token,
                ReleaseRequest {
                    artifact_id,
                    destinations: vec![destination],
                    environments: vec![],
                    force: false,
                    use_pipeline: false,
                    prepare_only: false,
                },
            ))
            .await?;

        let intents = resp.into_inner().intents;
        if let Some(intent) = intents.first() {
            self.data_mut().release_intent_id = intent.release_intent_id.clone();
        }

        Ok(self)
    }

    async fn release_reaches_terminal_state(self) -> anyhow::Result<Self> {
        let mut release_client = self.fixture().releases();
        let (token, release_intent_id) = {
            let data = self.data();
            (data.auth_token.clone(), data.release_intent_id.clone())
        };

        let terminal_status = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            async {
                loop {
                    let resp = release_client
                        .wait_release(authed_request(
                            &token,
                            WaitReleaseRequest {
                                release_intent_id: release_intent_id.clone(),
                            },
                        ))
                        .await;

                    match resp {
                        Ok(stream) => {
                            let mut stream = stream.into_inner();
                            while let Some(event) =
                                stream.message().await.expect("stream message")
                            {
                                if let Some(wait_release_event::Event::StatusUpdate(update)) =
                                    event.event
                                {
                                    let status = update.status.as_str();
                                    if matches!(
                                        status,
                                        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT"
                                    ) {
                                        return update.status;
                                    }
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                    }
                }
            },
        )
        .await?;

        self.data_mut().terminal_status = terminal_status;

        Ok(self)
    }
}
