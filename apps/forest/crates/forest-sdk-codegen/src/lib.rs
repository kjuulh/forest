use std::path::Path;

use crate::errors::{CodegenResult, Error};

pub mod codegen;
pub mod errors;
pub mod ir;
pub mod lower;
pub mod openapi;

pub struct Codegen {
    pub options: CodegenOptions,
}

pub struct CodegenOptions {
    pub destination: String,
    pub language: CodegenLanguage,
}

pub enum CodegenLanguage {
    Rust,
    TypeScript,
}

impl Codegen {
    pub fn generate(&self, input: &str) -> CodegenResult<String> {
        let doc = openapi::parse(input)?;

        let module = lower::lower(&doc)?;

        let output = codegen::emit(&module, &self.options.language)?;

        Ok(output)
    }

    pub fn generate_client(&self, input: &str, component_id: &str) -> CodegenResult<String> {
        let doc = openapi::parse(input)?;
        let module = lower::lower(&doc)?;
        codegen::emit_client(&module, &self.options.language, component_id)
    }

    pub async fn generate_for_file(&self, path: impl AsRef<Path>) -> CodegenResult<String> {
        let file_content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| Error::FileNotFound {
                    path: path.as_ref().to_path_buf(),
                    error: e,
                })?;

        self.generate(&file_content)
    }
}
