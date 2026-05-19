use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{Duration, Utc};

use super::{SessionData, SessionError, SessionId, SessionStore};

/// In-memory session store. Suitable for single-instance deployments.
/// Sessions are lost on server restart.
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<SessionId, SessionData>>,
    max_inactive: Duration,
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            max_inactive: Duration::days(30),
        }
    }

    /// Remove sessions inactive for longer than `max_inactive`.
    pub fn reap_expired(&self) {
        let cutoff = Utc::now() - self.max_inactive;
        let mut sessions = self.sessions.write().unwrap();
        sessions.retain(|_, data| data.last_seen_at > cutoff);
    }

    pub fn session_count(&self) -> usize {
        self.sessions.read().unwrap().len()
    }
}

#[async_trait::async_trait]
impl SessionStore for InMemorySessionStore {
    async fn create(&self, data: SessionData) -> Result<SessionId, SessionError> {
        let id = SessionId::generate();
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(id.clone(), data);
        Ok(id)
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionData>, SessionError> {
        let sessions = self.sessions.read().unwrap();
        Ok(sessions.get(id).cloned())
    }

    async fn update(&self, id: &SessionId, data: SessionData) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(id.clone(), data);
        Ok(())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(id);
        Ok(())
    }
}
