//! Cargo discovery: `cargo metadata` parsing plus blast-radius annotation.

mod blast;
mod metadata;

pub(crate) use metadata::discover;
