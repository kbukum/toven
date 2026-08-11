//! Per-target release tag grammar.
//!
//! The [`TagScheme`] codec is owned by the pure [`toven_semver`] toolkit; this
//! port surface re-exports it so adapters keep referring to
//! `toven_ports::TagScheme`.

pub use toven_semver::TagScheme;
