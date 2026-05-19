use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("file not found: {path}, error: {error}")]
    FileNotFound {
        path: PathBuf,
        error: tokio::io::Error,
    },

    #[error("failed to parse OpenAPI document: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("lowering error: {0}")]
    LoweringError(String),

    #[error("code generation error: {0}")]
    CodegenError(String),
}

impl From<std::fmt::Error> for Error {
    fn from(e: std::fmt::Error) -> Self {
        Error::CodegenError(e.to_string())
    }
}

pub type CodegenResult<T> = Result<T, Error>;
