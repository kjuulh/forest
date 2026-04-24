//! In-memory registry of connected hollow-agents and their capacity.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hollow_grpc_interface::{AgentRegister, ControllerMessage, RunJob, controller_message};
use tokio::sync::mpsc;

use crate::dispatcher::DEFAULT_VCPUS_PER_JOB;

struct ConnectedAgent {
    info: AgentRegister,
    tx: mpsc::UnboundedSender<ControllerMessage>,
    active_jobs: u32,
}

struct AgentPoolInner {
    agents: HashMap<String, ConnectedAgent>,
}

/// Thread-safe, cheaply cloneable pool of connected agents.
#[derive(Clone)]
pub struct AgentPool {
    inner: Arc<Mutex<AgentPoolInner>>,
}

impl AgentPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(AgentPoolInner {
                agents: HashMap::new(),
            })),
        }
    }

    /// Register a new agent. Returns false if the agent ID is already taken.
    pub fn register(
        &self,
        info: AgentRegister,
        tx: mpsc::UnboundedSender<ControllerMessage>,
    ) -> bool {
        let mut inner = self.inner.lock().expect("agent pool lock poisoned");
        let id = info.agent_id.clone();
        if inner.agents.contains_key(&id) {
            return false;
        }
        inner.agents.insert(
            id,
            ConnectedAgent {
                info,
                tx,
                active_jobs: 0,
            },
        );
        true
    }

    /// Remove an agent from the pool.
    pub fn remove(&self, agent_id: &str) {
        self.inner
            .lock()
            .expect("agent pool lock poisoned")
            .agents
            .remove(agent_id);
    }

    /// Try to dispatch a job to an agent with available capacity.
    /// Returns the agent_id if dispatched, None if no capacity.
    pub fn dispatch_job(&self, job: RunJob) -> Option<String> {
        let mut inner = self.inner.lock().expect("agent pool lock poisoned");

        // Best-fit: pick agent with most spare vCPU capacity
        let vcpus_per_job = DEFAULT_VCPUS_PER_JOB;
        let best = inner
            .agents
            .iter()
            .filter(|(_, a)| a.info.total_vcpus >= (a.active_jobs + 1) * vcpus_per_job)
            .max_by_key(|(_, a)| {
                a.info
                    .total_vcpus
                    .saturating_sub(a.active_jobs * vcpus_per_job)
            });

        if let Some((id, agent)) = best {
            let id = id.clone();
            let msg = ControllerMessage {
                message: Some(controller_message::Message::RunJob(job)),
            };
            if agent.tx.send(msg).is_ok() {
                if let Some(a) = inner.agents.get_mut(&id) {
                    a.active_jobs += 1;
                }
                return Some(id);
            }
        }
        None
    }

    /// Mark a job as complete on an agent, freeing capacity.
    pub fn job_completed(&self, agent_id: &str) {
        let mut inner = self.inner.lock().expect("agent pool lock poisoned");
        if let Some(agent) = inner.agents.get_mut(agent_id) {
            agent.active_jobs = agent.active_jobs.saturating_sub(1);
        }
    }

    /// Total active jobs across all agents.
    pub fn active_job_count(&self) -> u32 {
        self.inner
            .lock()
            .expect("agent pool lock poisoned")
            .agents
            .values()
            .map(|a| a.active_jobs)
            .sum()
    }
}
