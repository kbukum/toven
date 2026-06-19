//! Shared port test doubles, defined once and reused across all later steps.
//!
//! Decomposed by port (principles §4): the Provider seam, the VCS seam, and the
//! Reporter sink each own a file. Use these instead of redeclaring bespoke fakes
//! inside a crate.

mod provider;
mod reporter;
mod vcs;

pub use provider::{FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget};
pub use reporter::RecordingReporter;
pub use vcs::{FakeVcsReader, FakeVcsWriter, VcsWrite};
