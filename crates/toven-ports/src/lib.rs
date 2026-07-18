//! `toven-ports` — the port contracts every adapter (in-tree or 3rd-party)
//! implements, plus the fat helpers that make implementing them easy.
//!
//! Layer 1 of the hexagonal architecture: the thin traits ecosystems implement +
//! the shared surface behind them. Adapters build against `toven-ports`, never
//! against the engine. It depends only on [`toven_model`] (the shared
//! vocabulary), the error contract ([`rskit_errors`]), and the reuse primitives
//! it wraps ([`rskit_util`] templating, [`rskit_version`] semver).
//!
//! All fallible methods return [`rskit_errors::AppResult`]. Port traits are
//! object-safe so registries store trait objects (`dyn Provider`,
//! `dyn ConfiguredAdapter`, `dyn ReleaseTarget`, `dyn Reporter`,
//! `dyn RawOutputSink`, `dyn VcsReader`, `dyn VcsWriter`, `dyn ToolchainProber`,
//! `dyn SourceDigest`, `dyn CacheStore`).
//!
//! ## Ports
//! - [`provider`] — [`Provider`]/[`ConfiguredAdapter`]: the raw-TOML → configured
//!   adapter seam, plus the [`wizard`] onboarding steps.
//! - [`release`] — [`ReleaseTarget`] and friends: the thin ecosystem release sliver.
//! - [`reporter`] — [`Reporter`]: the observability output port.
//! - [`raw_output`] — [`RawOutputSink`]: the raw child-output sink port (sibling
//!   of [`Reporter`]; fed by the engine's `UnitOutputChannel`).
//! - [`vcs`] — [`VcsReader`]/[`VcsWriter`]: the single git seam.
//! - [`watch`] — [`WatchSource`]: the injected filesystem-watch seam (concrete
//!   rskit-fs adapter lives in the engine).
//! - [`toolchain`] — [`ToolchainProber`]: the injected toolchain-probe seam
//!   (concrete subprocess prober lives in the engine).
//! - [`source`] — [`SourceDigest`]: the injected content-digest seam (concrete
//!   filesystem digest lives in the engine).
//! - [`cache`] — [`CacheStore`] (read, PLAN) + [`CacheWriter`] (write, APPLY):
//!   the injected cache-record seam (concrete backend lives in the engine).
//! - [`exec`] — [`CommandRunner`]: the injected process-execution seam consumed
//!   by the APPLY wave walk (concrete `rskit-process` runner lives in the engine).
//! - [`discover`] — the discovery request/response vocabulary.
//! - [`driver`] — [`DriverLocator`]/[`DriverWizard`]: the out-of-process
//!   `toven-<eco>` driver seams (concrete adapters live in the engine).
//!
//! ## Shared surface
//! - [`task`] — the tasks vocabulary ([`Task`], [`TaskKind`], [`FanOut`], …).
//! - [`config`] — [`CommonEcosystemConfig`] (the `#[serde(flatten)]` target) + knobs.
//! - [`wizard`] — the data-only onboarding vocabulary ([`Detection`],
//!   [`Questionnaire`], [`Answers`]).
//! - [`template`] — [`CommandTemplate`] argv rendering over rskit-util.
//! - [`merge`] — the [`merge_task`] field-merge helper.

pub mod cache;
pub mod config;
pub mod discover;
pub mod driver;
pub mod exec;
pub mod merge;
pub mod provider;
pub mod raw_output;
pub mod release;
pub mod reporter;
pub mod source;
pub mod task;
pub mod template;
pub mod toolchain;
pub mod vcs;
pub mod watch;
pub mod wizard;

pub use cache::{CacheStore, CacheWriter};
pub use config::{
    BumpLevel, ChangelogConfig, CommonEcosystemConfig, DependentVersion, HooksConfig, HostConfig,
    PrereleaseConfig, ReleaseConfig, RunStrategy, SignConfig, TaskEntry, TaskOverride,
};
pub use discover::{DISCOVERY_SCHEMA_VERSION, DiscoverContext, DiscoverRequest, DiscoverResponse};
pub use driver::{DriverLocator, DriverWizard};
pub use exec::{
    CommandRunner, HeldProcess, Invocation, InvocationEnvPolicy, InvocationEnvironment,
    OutputObserver, RunOutcome, StartOutcome,
};
pub use merge::{merge_release, merge_task};
pub use provider::{ConfiguredAdapter, EcosystemFragment, Provider};
pub use raw_output::RawOutputSink;
pub use release::{
    Artifact, HostReleaseOutcome, HostedRelease, PublishOutcome, RegistryCadence, ReleaseAsset,
    ReleaseHost, ReleaseMutation, ReleaseTarget, TagScheme,
};
pub use reporter::Reporter;
pub use source::SourceDigest;
pub use task::{
    DEFAULT_READINESS_TIMEOUT, FanOut, Readiness, Task, TaskIntent, TaskKind, TaskOrigin,
    ToolchainProbe,
};
pub use template::{CommandTemplate, ReleaseVar, TaskVar};
pub use toolchain::ToolchainProber;
pub use vcs::{
    BaselineMode, BaselineSpec, ChangeRecord, ChangeStatus, Oid, TagRef, VcsReader, VcsWriter,
};
pub use watch::{ChangeBatch, ChangeBatchStream, WatchSource};
pub use wizard::{
    Answer, AnswerProvider, Answers, Detection, Question, QuestionId, QuestionKind, Questionnaire,
    TextRule,
};

#[cfg(test)]
mod object_safety;
