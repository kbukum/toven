//! Shared port test doubles, defined once and reused across all later steps.
//!
//! Decomposed by port: the Provider seam, the VCS seam, the
//! Reporter sink, the raw-output sink, and the injected toolchain/source/cache
//! IO ports each own a file. Use these instead of redeclaring bespoke fakes
//! inside a crate.

mod cache;
mod exec;
mod provider;
mod raw_output;
mod reporter;
mod source;
mod toolchain;
mod vcs;

pub use cache::{FakeCacheStore, RecordingCacheStore, RecordingCacheWriter};
pub use exec::FakeCommandRunner;
pub use provider::{FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget};
pub use raw_output::RecordingRawOutputSink;
pub use reporter::RecordingReporter;
pub use source::{EMPTY_IDENTITY, FakeSourceDigest};
pub use toolchain::CountingToolchainProber;
pub use vcs::{FakeVcsReader, FakeVcsWriter, VcsWrite};
