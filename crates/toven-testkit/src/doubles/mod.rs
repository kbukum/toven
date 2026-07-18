//! Shared port test doubles, defined once and reused across crates.
//!
//! Decomposed by port: the Provider seam, the VCS seam, the
//! Reporter sink, the raw-output sink, and the injected toolchain/source/cache
//! IO ports each own a file. Use these instead of redeclaring bespoke fakes
//! inside a crate.

mod answers;
mod cache;
mod driver;
mod exec;
mod host;
mod provider;
mod raw_output;
mod release;
mod reporter;
mod source;
mod toolchain;
mod vcs;
mod watch;

pub use answers::ScriptedAnswers;
pub use cache::{FakeCacheStore, RecordingCacheStore, RecordingCacheWriter};
pub use driver::{FakeDriverLocator, FakeDriverWizard};
pub use exec::FakeCommandRunner;
pub use host::{FakeReleaseHost, HostCall};
pub use provider::{FakeConfiguredAdapter, FakeProvider};
pub use raw_output::RecordingRawOutputSink;
pub use release::{FakeReleaseTarget, ReleaseCall};
pub use reporter::RecordingReporter;
pub use source::{EMPTY_IDENTITY, FakeSourceDigest};
pub use toolchain::CountingToolchainProber;
pub use vcs::{FakeVcsReader, FakeVcsWriter, VcsWrite};
pub use watch::{ScriptedWatchSource, WatchCall};
