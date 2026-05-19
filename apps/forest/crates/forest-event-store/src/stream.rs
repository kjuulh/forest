/// Expected stream version for optimistic concurrency on append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedVersion {
    /// Stream must not exist yet (first write).
    NoStream,
    /// Stream must be at exactly this version.
    Exact(i64),
    /// No concurrency check — always append.
    Any,
}

/// Direction for reading events from a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDirection {
    Forward,
    Backward,
}

/// Query parameters for reading events.
#[derive(Debug, Clone)]
pub struct StreamQuery {
    pub direction: ReadDirection,
    pub from_version: i64,
    pub limit: i64,
}

impl Default for StreamQuery {
    fn default() -> Self {
        Self {
            direction: ReadDirection::Forward,
            from_version: 0,
            limit: 1000,
        }
    }
}
