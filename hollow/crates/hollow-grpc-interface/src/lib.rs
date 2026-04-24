#![allow(clippy::empty_docs, clippy::large_enum_variant)]

#[path = "./grpc/hollow/v1/hollow.v1.rs"]
pub mod grpc;

pub use grpc::*;
