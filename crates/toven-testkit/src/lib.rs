//! `toven-testkit` — the one shared, dependency-light test-support surface every
//! Toven crate's tests build on.
//!
//! Layer note: this is a **dev-support crate**. It may depend on
//! [`toven_model`] and [`toven_ports`], but **nothing ships it** and no
//! production crate depends on it (`publish = false`). It exists so steps 3–14
//! *consume* one fixture tree + one set of port doubles instead of re-inventing
//! fixtures, re-declaring port fakes, or embedding TOML inline.
//!
//! ## What lives here
//! - [`fixtures`] — typed loaders rooted at this crate's `fixtures/` tree
//!   ([`fixtures::document`], [`fixtures::ecosystem`], [`fixtures::repo_path`]),
//!   with clear errors on a missing/renamed fixture.
//! - [`workspace`] — a [`TestWorkspace`] pointed at the shared fixture root.
//! - [`repo`] — [`SampleRepo`]: materialize a `repos/<name>` tree into a temp dir
//!   and optionally `git init` it.
//! - [`git`] — git-scenario helpers ([`GitScenario`](git::GitScenario)) over `rskit-git`.
//! - [`doubles`] — the shared port doubles ([`FakeProvider`],
//!   [`FakeConfiguredAdapter`], [`FakeVcsReader`], [`FakeVcsWriter`],
//!   [`RecordingReporter`], [`RecordingRawOutputSink`], [`CountingToolchainProber`],
//!   [`FakeSourceDigest`], [`FakeCacheStore`], [`RecordingCacheStore`]).
//! - [`assertions`] — rskit `assert_ok`/`assert_err_code` re-exports plus
//!   Toven-domain event assertions.
//!
//! ## Reuse, don't re-implement
//! The temp-dir + fixture harness is `rskit-testutil`'s
//! [`TestWorkspace`]; safe paths come from
//! `rskit-fs`; git scripting goes through `rskit-git`. This crate adds only the
//! Toven-shaped layer on top.
#![warn(missing_docs)]

pub mod assertions;
pub mod doubles;
pub mod fixtures;
pub mod git;
pub mod repo;
pub mod workspace;

pub use assertions::{
    assert_emitted, assert_err_code, assert_event_sequence, assert_ok, find_event,
};
pub use doubles::{
    CountingToolchainProber, EMPTY_IDENTITY, FakeCacheStore, FakeCommandRunner,
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeSourceDigest, FakeVcsReader,
    FakeVcsWriter, RecordingCacheStore, RecordingCacheWriter, RecordingRawOutputSink,
    RecordingReporter, ReleaseCall, VcsWrite,
};
pub use fixtures::{
    FIXTURES_ROOT, document, document_path, document_string, ecosystem, ecosystem_string, repo_path,
};
pub use repo::SampleRepo;
pub use rskit_testutil::{CurrentDirGuard, TestWorkspace};
pub use workspace::fixtures_root;
