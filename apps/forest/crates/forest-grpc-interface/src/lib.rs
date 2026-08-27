#![allow(clippy::empty_docs, clippy::large_enum_variant)]

#[path = "./grpc/forest/v1/forest.v1.rs"]
pub mod grpc;

pub use grpc::*;

/// The `forest.provider.v1` contract an external destination provider implements.
///
/// Deliberately a separate package with no imports: a provider vendors the one
/// `.proto` file and needs nothing else from forest.
#[path = "./grpc/forest/provider/v1/forest.provider.v1.rs"]
pub mod provider;
