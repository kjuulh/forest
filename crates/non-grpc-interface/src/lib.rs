#![allow(clippy::empty_docs)]

pub mod grpc {
    include!("./grpc/non.v1.rs");
}

pub use grpc::*;
