//! `release verify` verb: check that release artifacts are present, authentic, and run.

mod adapters;
mod assets;
mod flow;
#[cfg(test)]
mod tests;

pub use adapters::{CosignVerifier, GhAssetDownloader, ProcessVersionProbe};
pub use flow::{VerifiedAsset, VerifyMode, VerifyOptions, VerifyReport, release_verify};
