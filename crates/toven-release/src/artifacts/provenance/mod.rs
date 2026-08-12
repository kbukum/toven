//! Provenance release phase and `gh attestation` adapter.

mod attestation;
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod phase;
#[cfg(test)]
mod tests;

pub use attestation::GhAttestationProvenance;
pub use phase::{
    ProvenanceOptions, ProvenancePhaseStatus, ProvenanceReport, ProvenanceSubjectReport,
    release_provenance,
};
#[cfg(test)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) use toven_ports::ToolOutcome;
