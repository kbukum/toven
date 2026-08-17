//! `release verify` verb: check that release artifacts are present, authentic, and run.

mod adapters;
mod assets;
mod flow;
#[cfg(test)]
mod tests;

pub use adapters::{CosignVerifier, GhAssetDownloader, ProcessVersionProbe};
pub use flow::{
    VerifiedAsset, VerifyInputs, VerifyMode, VerifyOperation, VerifyOptions, VerifyOutcome,
    VerifyReport, release_verify, verify_operation,
};
