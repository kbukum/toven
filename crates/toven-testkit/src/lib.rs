//! `toven-testkit` — the one shared, dependency-light test-support surface
//! every Toven crate's tests build on.
//!
//! Layer note: this is a **dev-support crate**. It may depend on
//! [`toven_model`] and [`toven_ports`], but **nothing ships it** and no
//! production crate depends on it (`publish = false`). It exists so steps 3–14
//! *consume* one fixture tree + one set of port doubles instead of re-inventing
//! fixtures, re-declaring port fakes, or embedding TOML inline.
//!
//! ## What lives here
//! - [`fixtures`] — typed loaders rooted at this crate's `fixtures/` tree
//!   ([`fixtures::document`], [`fixtures::ecosystem`],
//!   [`fixtures::repo_path`]), with clear errors on a missing/renamed fixture.
//! - [`workspace`] — a [`TestWorkspace`] pointed at the shared fixture root.
//! - [`repo`] — [`SampleRepo`]: materialize a `repos/<name>` tree into a temp
//!   dir and optionally `git init` it.
//! - [`git`] — git-scenario helpers ([`GitScenario`](git::GitScenario)) over
//!   `rskit-git`.
//! - [`scenario`] — the data-driven golden system: the declarative scenario
//!   schema ([`Scenario`]), typed loader, discovery, and the engine
//!   ([`scenario::run_scenario`]) that materializes, git-scripts, runs, and
//!   verifies a whole session.
//! - [`exec`] — the one shared blocking spawn path (rskit-process) under both
//!   the smoke and scenario harnesses.
//! - [`smoke`] — the shared end-to-end smoke harness ([`RunResult`], [`run`],
//!   [`run_ok`]) every app's `tests/smoke*.rs` drives the real binary through.
//! - [`doubles`] — the shared port doubles ([`FakeProvider`],
//!   [`FakeConfiguredAdapter`], [`FakeVcsReader`], [`FakeVcsWriter`],
//!   [`RecordingReporter`], [`RecordingRawOutputSink`],
//!   [`CountingToolchainProber`], [`FakeSourceDigest`], [`FakeCacheStore`],
//!   [`RecordingCacheStore`]).
//! - [`assertions`] — rskit `assert_ok`/`assert_err_code` re-exports plus
//!   Toven-domain event assertions.
//!
//! ## Reuse, don't re-implement
//! The temp-dir + fixture harness is `rskit-testutil`'s [`TestWorkspace`]; safe
//! paths come from `rskit-fs`; git scripting goes through `rskit-git`. This
//! crate adds only the Toven-shaped layer on top.
#![warn(missing_docs)]

pub mod assertions;
pub mod doubles;
pub mod exec;
pub mod fixtures;
pub mod git;
pub mod repo;
pub mod scenario;
pub mod smoke;
pub mod workspace;

pub use assertions::{
    assert_emitted, assert_err_code, assert_event_sequence, assert_ok, find_event,
};
pub use doubles::{
    CountingToolchainProber, EMPTY_IDENTITY, FakeAssetDownloader, FakeCacheStore,
    FakeCommandRunner, FakeConfiguredAdapter, FakeDelegatedPhase, FakeDriverLocator,
    FakeDriverWizard, FakeProvider, FakeReleaseHost, FakeReleaseTarget, FakeSignatureVerifier,
    FakeSigner, FakeSourceDigest, FakeVcsReader, FakeVcsWriter, FakeVersionProbe, HookCall,
    HostCall, RecordingCacheStore, RecordingCacheWriter, RecordingHookRunner,
    RecordingRawOutputSink, RecordingReporter, ReleaseCall, ScriptedAnswers,
    ScriptedToolchainProber, ScriptedWatchSource, SignerCall, VcsWrite, VerifyCall, WatchCall,
};
pub use fixtures::{
    FIXTURES_ROOT, coverage_profile_string, document, document_path, document_string, ecosystem,
    ecosystem_string, raw_subtree, repo_path, scenario_path,
};
pub use repo::SampleRepo;
pub use rskit_testutil::{CurrentDirGuard, TestWorkspace};
pub use scenario::{Effect, Scenario, Step};
pub use smoke::{CLOCK_EPOCH_ENV, CLOCK_EPOCH_VALUE, RunResult, program_on_path, run, run_ok};
pub use workspace::fixtures_root;
