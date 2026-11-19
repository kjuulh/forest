#[rustfmt::skip]
#[allow(clippy::all, unused)]
mod compiled {
    include!(concat!(env!("OUT_DIR"), "/sedge.rs"));
}
pub use compiled::*;

pub mod chrome;
pub mod models;
pub mod release;
pub mod org_settings;
