//! Shared application state for the hollow-controller.
//! Follows the *State trait pattern used by forest-server:
//! - State holds raw resources (connections, config)
//! - *State traits provide accessor methods to construct services
//! - OnceLock for application-lifetime singletons

use std::sync::OnceLock;

use crate::agent_pool::AgentPool;
use crate::job_tracker::JobTracker;

#[derive(Clone)]
pub struct State {
    pub server_addr: String,
}

impl State {
    pub fn new(server_addr: String) -> Self {
        Self { server_addr }
    }
}

// -- AgentPool: application-lifetime singleton --

pub trait AgentPoolState {
    fn agent_pool(&self) -> AgentPool;
}

impl AgentPoolState for State {
    fn agent_pool(&self) -> AgentPool {
        static ONCE: OnceLock<AgentPool> = OnceLock::new();
        ONCE.get_or_init(AgentPool::new).clone()
    }
}

// -- JobTracker: application-lifetime singleton --

pub trait JobTrackerState {
    fn job_tracker(&self) -> JobTracker;
}

impl JobTrackerState for State {
    fn job_tracker(&self) -> JobTracker {
        static ONCE: OnceLock<JobTracker> = OnceLock::new();
        ONCE.get_or_init(JobTracker::new).clone()
    }
}
