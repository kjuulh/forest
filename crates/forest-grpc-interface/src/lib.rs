#![allow(clippy::empty_docs)]

pub mod grpc {
    include!("./grpc/forest/v1/forest.v1.rs");
}

pub use grpc::*;
