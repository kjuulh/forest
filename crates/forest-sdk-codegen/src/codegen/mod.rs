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
