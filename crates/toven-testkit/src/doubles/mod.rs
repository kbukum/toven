//! Shared port test doubles, defined once and reused across crates.
//!
//! Decomposed by port: the Provider seam, the VCS seam, the Reporter sink, the
//! raw-output sink, and the injected toolchain/source/cache IO ports each own a
//! file. Use these instead of redeclaring bespoke fakes inside a crate.

mod answers;
mod cache;
mod delegated;
mod driver;
mod exec;
mod hook;
mod host;
mod image;
mod provenance;
mod provider;
mod raw_output;
mod release;
mod reporter;
mod signer;
mod source;
mod toolchain;
mod vcs;
mod verify;
mod watch;

pub use answers::ScriptedAnswers;
pub use cache::{FakeCacheStore, RecordingCacheStore, RecordingCacheWriter};
pub use delegated::FakeDelegatedPhase;
pub use driver::{FakeDriverLocator, FakeDriverWizard};
pub use exec::FakeCommandRunner;
pub use hook::{HookCall, RecordingHookRunner, ResolvedCall, ScriptedResolvedRunner};
pub use host::{FakeReleaseHost, HostCall};
pub use image::{FakeImagePhase, ImageCall};
pub use provenance::{FakeProvenancePhase, ProvenanceCall};
pub use provider::{FakeConfiguredAdapter, FakeProvider};
pub use raw_output::RecordingRawOutputSink;
pub use release::{FakeReleaseTarget, ReleaseCall};
pub use reporter::RecordingReporter;
pub use signer::{FakeSigner, SignerCall};
pub use source::{EMPTY_IDENTITY, FakeSourceDigest};
pub use toolchain::{CountingToolchainProber, ScriptedToolchainProber};
pub use vcs::{FakeVcsReader, FakeVcsWriter, VcsWrite};
pub use verify::{FakeAssetDownloader, FakeSignatureVerifier, FakeVersionProbe, VerifyCall};
pub use watch::{ScriptedWatchSource, WatchCall};
