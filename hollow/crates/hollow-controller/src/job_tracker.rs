//! Tracks in-flight jobs and provides a channel for the gRPC server to forward
//! agent events (logs, completion) back to the dispatcher for forest-server reporting.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

/// Events from an agent about a specific job, forwarded to the dispatcher.
#[derive(Debug)]
pub enum JobEvent {
    Log {
        channel: String,
        line: String,
        timestamp: u64,
    },
    Completed {
        exit_code: i32,
        plan_output: Option<String>,
    },
    Failed {
        error_message: String,
    },
}

/// Handle held by the dispatcher to receive events for a specific job.
pub struct JobHandle {
    pub rx: mpsc::UnboundedReceiver<JobEvent>,
}

struct JobEntry {
    tx: mpsc::UnboundedSender<JobEvent>,
}

struct JobTrackerInner {
    jobs: HashMap<String, JobEntry>,
}

/// Shared state tracking all in-flight jobs. Cheaply cloneable.
#[derive(Clone)]
pub struct JobTracker {
    inner: Arc<Mutex<JobTrackerInner>>,
}

impl JobTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(JobTrackerInner {
                jobs: HashMap::new(),
            })),
        }
    }

    /// Register a new job. Returns a handle for the dispatcher to receive events.
    pub fn register_job(
        &self,
        job_id: String,
        _release_token: String,
        _agent_id: String,
    ) -> JobHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner
            .lock()
            .expect("job tracker lock poisoned")
            .jobs
            .insert(job_id, JobEntry { tx });
        JobHandle { rx }
    }

    /// Send a log event for a job. Returns false if the job is not tracked.
    pub fn send_log(&self, job_id: &str, channel: String, line: String, timestamp: u64) -> bool {
        let inner = self.inner.lock().expect("job tracker lock poisoned");
        if let Some(entry) = inner.jobs.get(job_id) {
            entry
                .tx
                .send(JobEvent::Log {
                    channel,
                    line,
                    timestamp,
                })
                .is_ok()
        } else {
            false
        }
    }

    /// Send a completion event for a job and remove it from tracking.
    pub fn send_completed(&self, job_id: &str, exit_code: i32, plan_output: Option<String>) {
        let mut inner = self.inner.lock().expect("job tracker lock poisoned");
        if let Some(entry) = inner.jobs.remove(job_id) {
            let _ = entry.tx.send(JobEvent::Completed {
                exit_code,
                plan_output,
            });
        }
    }

    /// Send a failure event for a job and remove it from tracking.
    pub fn send_failed(&self, job_id: &str, error_message: String) {
        let mut inner = self.inner.lock().expect("job tracker lock poisoned");
        if let Some(entry) = inner.jobs.remove(job_id) {
            let _ = entry.tx.send(JobEvent::Failed { error_message });
        }
    }
}
