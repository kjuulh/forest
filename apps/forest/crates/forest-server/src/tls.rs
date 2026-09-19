//! Process-level rustls setup.
//!
//! Forest's dependency graph enables both rustls crypto providers on rustls
//! 0.23: `async-nats` pulls `ring`, while `rust-s3`/`attohttpc` and `reqwest`
//! pull `aws-lc-rs`. With more than one provider compiled in, rustls will not
//! pick one on its own — it panics on the first handshake with
//! "Could not automatically determine the process-level CryptoProvider".
//!
//! Every entry point that opens a TLS connection (Aurora over `sslmode=require`
//! or `verify-ca`, S3, OTLP) must therefore install a provider first. We pin the
//! pure-Rust `ring` provider in preference to the `aws-lc-sys` C wrapper.

/// Install `ring` as the process-level rustls crypto provider.
///
/// Idempotent and safe to call from every binary: a provider can only be
/// installed once per process, and a losing race means an equivalent provider
/// is already in place.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
