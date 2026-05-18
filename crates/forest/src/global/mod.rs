//! Forest global-tools subsystem.
//!
//! Implements TASKS/018-global-tools.md.
//!
//! Architecture: this module is split into a **pure core** (deterministic,
//! no I/O) and an **effectful shell** (the only code allowed to touch the
//! filesystem, network, or process boundary). The pure modules must not
//! import the shell modules; the spec's §1b.1 purity boundary is enforced
//! by a CI gate.
//!
//! Pure core:
//!   - [`names`] — tool/shim name validation.
//!   - [`shim`] — shim script generation.
//!   - [`lockfile`] — strict-mode global lockfile parser/serialiser.
//!
//! Effectful shell modules will be added as implementation progresses.

pub mod eval;
pub mod extract;
pub mod lockfile;
pub mod manifest;
pub mod names;
pub mod resolver;
pub mod shim;
pub mod user_config;

// --- Effectful shell ---
pub mod cache;
pub mod cue_eval;
pub mod fs;
pub mod paths;
pub mod platform;
pub mod service;
