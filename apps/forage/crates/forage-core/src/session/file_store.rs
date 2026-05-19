use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};

use super::{SessionData, SessionError, SessionId, SessionStore};

/// File-based session store. Each session is a JSON file in a directory.
/// Suitable for local development — sessions survive server restarts.
pub struct FileSessionStore {
    dir: PathBuf,
    max_inactive: Duration,
}

impl FileSessionStore {
    pub fn new(dir: impl AsRef<Path>) -> Result<Self, SessionError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .map_err(|e| SessionError::Store(format!("failed to create session dir: {e}")))?;
        Ok(Self {
            dir,
            max_inactive: Duration::days(30),
        })
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        // Use a safe filename: replace any non-alphanumeric chars
        let safe_name: String = id
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.dir.join(format!("{safe_name}.json"))
    }

    /// Remove sessions inactive for longer than `max_inactive`.
    pub fn reap_expired(&self) {
        let cutoff = Utc::now() - self.max_inactive;
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(data) = serde_json::from_str::<SessionData>(&contents) {
                    if data.last_seen_at < cutoff {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
    }

    pub fn session_count(&self) -> usize {
        std::fs::read_dir(&self.dir)
            .map(|e| {
                e.flatten()
                    .filter(|e| {
                        e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            == Some("json")
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn create(&self, data: SessionData) -> Result<SessionId, SessionError> {
        let id = SessionId::generate();
        let path = self.session_path(&id);
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| SessionError::Store(format!("serialize error: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| SessionError::Store(format!("write error: {e}")))?;
        Ok(id)
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionData>, SessionError> {
        let path = self.session_path(id);
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                let data: SessionData = serde_json::from_str(&contents)
                    .map_err(|e| SessionError::Store(format!("deserialize error: {e}")))?;
                Ok(Some(data))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SessionError::Store(format!("read error: {e}"))),
        }
    }

    async fn update(&self, id: &SessionId, data: SessionData) -> Result<(), SessionError> {
        let path = self.session_path(id);
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| SessionError::Store(format!("serialize error: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| SessionError::Store(format!("write error: {e}")))?;
        Ok(())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionError> {
        let path = self.session_path(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SessionError::Store(format!("delete error: {e}"))),
        }
    }
}
