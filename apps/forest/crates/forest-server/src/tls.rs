//! Process-level rustls setup.
//!
//! `ring` is now the only provider compiled into rustls 0.23 — see the comment
//! on the `rustls` pin in the workspace manifest for how the graph is held to
//! that. rustls can therefore pick it unaided, and this call is no longer the
//! thing standing between a binary and a panicking handshake.
//!
//! It stays because it is what makes the choice explicit and cheap to keep: if
//! a future dependency enables `aws-lc-rs`, rustls goes back to refusing to
//! guess, and every entry point that opens TLS (Aurora over `sslmode=require`
//! or `verify-ca`, S3, OTLP) would start panicking. With this call in place
//! they keep working, on `ring`, and the regression shows up as `aws-lc-sys`
//! appearing in the build rather than as an outage.

/// Install `ring` as the process-level rustls crypto provider.
///
/// Idempotent and safe to call from every binary: a provider can only be
/// installed once per process, and a losing race means an equivalent provider
/// is already in place.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
