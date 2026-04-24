//! Shared vsock wire protocol between hollow-agent (host) and hollow-guest (VM).
//!
//! Uses a simple length-prefixed framing over AF_VSOCK or any AsyncRead/AsyncWrite stream:
//!
//! ```text
//! ┌──────────┬──────────┬─────────────┐
//! │ type(u8) │ len(u32) │ payload     │
//! └──────────┴──────────┴─────────────┘
//! ```
//!
//! All payloads are JSON-encoded for simplicity and debuggability.

pub mod protocol;
pub mod transport;
