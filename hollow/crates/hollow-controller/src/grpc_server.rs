//! gRPC server that agents connect to for registration and job dispatch.

use std::net::SocketAddr;
use std::pin::Pin;

use futures::{Stream, StreamExt};
use hollow_grpc_interface::{
    AgentMessage, AgentRegisterAck, ControllerMessage, agent_message, controller_message,
    hollow_agent_service_server::{HollowAgentService, HollowAgentServiceServer},
};
use notmad::{Component, ComponentInfo, MadError};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status, Streaming};

use crate::agent_pool::AgentPool;
use crate::job_tracker::JobTracker;
use crate::state::{AgentPoolState, JobTrackerState, State};

pub struct AgentGrpcServer {
    pool: AgentPool,
    tracker: JobTracker,
    listen_addr: SocketAddr,
}

pub trait AgentGrpcServerState {
    fn agent_grpc_server(&self, listen_addr: SocketAddr) -> AgentGrpcServer;
}

impl AgentGrpcServerState for State {
    fn agent_grpc_server(&self, listen_addr: SocketAddr) -> AgentGrpcServer {
        AgentGrpcServer {
            pool: self.agent_pool(),
            tracker: self.job_tracker(),
            listen_addr,
        }
    }
}

impl Component for AgentGrpcServer {
    fn info(&self) -> ComponentInfo {
        "hollow/agent-grpc-server".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        tracing::info!(addr = %self.listen_addr, "starting agent gRPC server");

        tonic::transport::Server::builder()
            .add_service(HollowAgentServiceServer::new(AgentServiceImpl {
                pool: self.pool.clone(),
                tracker: self.tracker.clone(),
            }))
            .serve_with_shutdown(self.listen_addr, cancellation_token.cancelled_owned())
            .await
            .map_err(|e| MadError::Inner(e.into()))?;

        Ok(())
    }
}

struct AgentServiceImpl {
    pool: AgentPool,
    tracker: JobTracker,
}

#[tonic::async_trait]
impl HollowAgentService for AgentServiceImpl {
    type RegisterAgentStream =
        Pin<Box<dyn Stream<Item = Result<ControllerMessage, Status>> + Send>>;

    async fn register_agent(
        &self,
        request: Request<Streaming<AgentMessage>>,
    ) -> Result<Response<Self::RegisterAgentStream>, Status> {
        let mut inbound = request.into_inner();

        let register = match inbound.next().await {
            Some(Ok(msg)) => match msg.message {
                Some(agent_message::Message::Register(r)) => r,
                _ => return Err(Status::invalid_argument("first message must be Register")),
            },
            Some(Err(e)) => return Err(e),
            None => return Err(Status::cancelled("stream closed before registration")),
        };

        let agent_id = register.agent_id.clone();
        tracing::info!(
            agent_id = %agent_id,
            hostname = %register.hostname,
            pool = %register.pool,
            vcpus = register.total_vcpus,
            memory_mib = register.total_memory_mib,
            images = ?register.available_images,
            "agent registering"
        );

        let (tx, rx) = mpsc::unbounded_channel::<ControllerMessage>();

        tx.send(ControllerMessage {
            message: Some(controller_message::Message::RegisterAck(AgentRegisterAck {
                agent_id: agent_id.clone(),
                accepted: true,
                reason: String::new(),
            })),
        })
        .map_err(|_| Status::internal("failed to send ack"))?;

        if !self.pool.register(register, tx) {
            return Err(Status::already_exists("agent ID already registered"));
        }

        tracing::info!(agent_id = %agent_id, "agent registered");
        metrics::gauge!(crate::metrics::names::AGENTS_CONNECTED).increment(1.0);

        let pool = self.pool.clone();
        let tracker = self.tracker.clone();
        let agent_id_clone = agent_id.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound.next().await {
                match msg {
                    Ok(msg) => handle_agent_message(&pool, &tracker, &agent_id_clone, msg),
                    Err(e) => {
                        tracing::warn!(agent_id = %agent_id_clone, error = %e, "agent stream error");
                        break;
                    }
                }
            }
            tracing::info!(agent_id = %agent_id_clone, "agent disconnected");
            pool.remove(&agent_id_clone);
            metrics::gauge!(crate::metrics::names::AGENTS_CONNECTED).decrement(1.0);
        });

        let outbound = UnboundedReceiverStream::new(rx).map(Ok);
        Ok(Response::new(Box::pin(outbound)))
    }
}

fn handle_agent_message(pool: &AgentPool, tracker: &JobTracker, agent_id: &str, msg: AgentMessage) {
    match msg.message {
        Some(agent_message::Message::Heartbeat(hb)) => {
            tracing::trace!(agent_id, active_vms = hb.active_vms, "heartbeat");
        }
        Some(agent_message::Message::JobUpdate(update)) => {
            let status = hollow_grpc_interface::JobStatus::try_from(update.status);
            tracing::info!(
                agent_id,
                job_id = %update.job_id,
                status = ?status,
                "job update"
            );

            match status {
                Ok(hollow_grpc_interface::JobStatus::Completed) => {
                    pool.job_completed(agent_id);
                    tracker.send_completed(&update.job_id, update.exit_code, update.plan_output);
                }
                Ok(
                    hollow_grpc_interface::JobStatus::Failed
                    | hollow_grpc_interface::JobStatus::TimedOut
                    | hollow_grpc_interface::JobStatus::Cancelled,
                ) => {
                    pool.job_completed(agent_id);
                    tracker.send_failed(&update.job_id, update.error_message);
                }
                _ => {}
            }
        }
        Some(agent_message::Message::LogBatch(batch)) => {
            for line in &batch.lines {
                tracker.send_log(
                    &batch.job_id,
                    line.channel.clone(),
                    line.line.clone(),
                    line.timestamp,
                );
            }
        }
        Some(agent_message::Message::Register(_)) => {
            tracing::warn!(agent_id, "unexpected re-register");
        }
        None => {}
    }
}
