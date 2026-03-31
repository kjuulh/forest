pub mod rust;
pub mod typescript;

use crate::errors::CodegenResult;
use crate::ir;
use crate::CodegenLanguage;

pub fn emit(module: &ir::Module, language: &CodegenLanguage) -> CodegenResult<String> {
    match language {
        CodegenLanguage::Rust => rust::emit(module),
        CodegenLanguage::TypeScript => typescript::emit(module),
    }
}

/// Generate a typed client for calling a dependency component.
pub fn emit_client(
    module: &ir::Module,
    language: &CodegenLanguage,
    component_id: &str,
) -> CodegenResult<String> {
    match language {
        CodegenLanguage::TypeScript => typescript::emit_client(module, component_id),
        CodegenLanguage::Rust => {
            // TODO: Rust client generation
            Err(crate::errors::Error::LoweringError(
                "Rust dependency client generation not yet supported".into(),
            ))
        }
    }
}
