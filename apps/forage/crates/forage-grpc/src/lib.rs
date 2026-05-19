#![allow(clippy::empty_docs)]
#![allow(clippy::large_enum_variant)]

#[path = "./grpc/forest/v1/forest.v1.rs"]
pub mod grpc;

pub use grpc::*;
