//! Read-only release status projection on the shared runtime engine.
//!
//! Reports, per releasable module, the version its manifest declares, the
//! newest release tag cut for it, and the versions the registry already reports
//! as published — all without mutating any manifest, tag, or registry.
//!
//! The verb splits into the canonical shared-GATHER → per-unit-STREAM shape:
//! [`StatusInputs::gather`] resolves the workspace-coupled prerequisites (the
//! release targets, resolved settings, and the per-member tag snapshot) exactly
//! once, and [`StatusOperation`] streams one module's declared/published/tag
//! verdict per unit on the [`toven_runtime`] engine, so the slow per-module
//! registry lookups run bounded-parallel and settle live rather than buffering a
//! terminal table. [`release_status`] retains the buffered aggregate for
//! programmatic callers, assembled from the same per-unit [`module_status`]
//! compute.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use tokio_util::sync::CancellationToken;
use toven_model::{EcosystemId, MemberId, Module, ModuleKey};
use toven_ports::{Provider, Reporter, TagRef};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use crate::ResolvedReleaseSettings;
use crate::model::tag;
use crate::{ReleaseModuleStatus, ReleaseStatus};
use toven_core::config::Document;
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

use crate::planning::plan::{release_targets, resolve_release_settings};

/// One releasable module's fully-owned status inputs, resolved during GATHER.
///
/// Carries everything `module_status` needs so the per-unit phase is a pure,
/// total function of gathered data — no borrow of the providers or VCS readers
/// survives into the streamed phase.
struct StatusModule {
    /// Stable unit id (the module's canonical key string).
    id: String,
    /// The module's canonical key.
    key: ModuleKey,
    /// The discovered module (its manifest path drives the declared-version read).
    module: Module,
    /// The train the module's release target is keyed under.
    member: Option<MemberId>,
    /// The train's ecosystem.
    ecosystem: EcosystemId,
    /// The module's resolved release settings (publication, offline, tag format…).
    settings: ResolvedReleaseSettings,
}

/// The shared, workspace-coupled prerequisites for `release status`, resolved
/// once by [`StatusInputs::gather`] and handed to every per-unit run.
pub struct StatusInputs {
    /// Release targets keyed by `(member, ecosystem)` train — thread-safe
    /// (`Send + Sync`) so they can be shared across the engine's worker pool.
    targets: crate::ReleaseTargets,
    /// Each member repo's tag snapshot, listed once per member.
    tags_by_member: BTreeMap<Option<MemberId>, Vec<TagRef>>,
    /// The releasable modules, in discovery order.
    modules: Vec<StatusModule>,
}

impl StatusInputs {
    /// Resolve the release targets, settings, and per-member tag snapshot once.
    ///
    /// A module is releasable when its ecosystem adapter exposes a release
    /// target and its resolved publication actually releases; others are
    /// omitted. The per-member tag set is listed once (mirroring change
    /// detection) rather than once per module.
    ///
    /// # Errors
    /// Propagates configuration/discovery/graph failures and VCS tag-listing
    /// failures.
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
        readers: &MemberVcsReaders<'_>,
        reporter: &mut dyn Reporter,
    ) -> AppResult<Self> {
        let locator = PathDriverLocator::new();
        let context = prepare_front(
            &request.project_root,
            document,
            providers,
            &locator,
            reporter,
        )?;
        let targets = release_targets(&context, readers)?;
        let settings = resolve_release_settings(&context, &targets)?;
        let tags_by_member = list_member_tags(readers)?;

        let mut modules = Vec::new();
        for module in &context.federation.modules {
            let key = (module.member.clone(), module.id.ecosystem.clone());
            if !targets.contains_key(&key) {
                continue;
            }
            let Some(resolved) = settings.get(&module.key()) else {
                continue;
            };
            if !resolved.publication.releases() {
                continue;
            }
            modules.push(StatusModule {
                id: module.key().to_string(),
                key: module.key(),
                module: module.clone(),
                member: module.member.clone(),
                ecosystem: module.id.ecosystem.clone(),
                settings: resolved.clone(),
            });
        }
        Ok(Self {
            targets,
            tags_by_member,
            modules,
        })
    }

    /// Look up a releasable module by its unit id.
    fn module(&self, id: &str) -> Option<&StatusModule> {
        self.modules.iter().find(|module| module.id == id)
    }

    /// The engine unit graph: one independent (edgeless) unit per releasable
    /// module, so the engine schedules them as a single bounded-parallel wave.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        self.modules
            .iter()
            .map(|module| UnitSpec::new(module.id.clone(), Vec::<String>::new()))
            .collect()
    }
}

/// Project one releasable module's declared/published/tagged state.
///
/// Pure over the gathered [`StatusInputs`]: the only I/O is the module's own
/// declared-version read and (online, registry-publishing modules) the
/// best-effort registry lookup — the slow call the engine streams per unit.
///
/// # Errors
/// Returns [`ErrorCode::Internal`] if the module's release target is missing
/// from the gathered set, and propagates release-target version I/O failures.
fn module_status(inputs: &StatusInputs, module: &StatusModule) -> AppResult<ReleaseModuleStatus> {
    let target = inputs
        .targets
        .get(&(module.member.clone(), module.ecosystem.clone()))
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("module '{}' has no gathered release target", module.key),
            )
        })?;
    let resolved = &module.settings;
    let declared = target.declared_version(&module.module)?;
    // Only a registry-published module in online mode has a meaningful
    // published set. A tag-only module never publishes (querying it would
    // still hit the network — e.g. `cargo search` — for a set that is
    // unused), and `offline` anchors idempotency on release tags instead of
    // registry queries. In both cases the projection stays truthful and
    // network-free by reporting no registry-published set rather than
    // querying one.
    let published = if resolved.offline || !resolved.publication.publishes_to_registry() {
        Vec::new()
    } else {
        target.published_versions(&module.module)?
    };
    let scheme = target.tag_scheme(&module.module, resolved.tag_format.as_deref())?;
    let latest = inputs
        .tags_by_member
        .get(&module.member)
        .and_then(|tags| tag::latest(&scheme, tags));
    // Offline there is no registry set to consult — idempotency anchors on
    // release tags instead (mirroring plan-time `planned <= tagged`), so the
    // published verdict comes from the newest release tag: a declared
    // version at/below it has already been released. A never-versioned module
    // has no declared version to compare, so it is simply not published yet.
    let is_published = if resolved.offline {
        declared.as_ref().is_some_and(|declared| {
            latest
                .as_ref()
                .is_some_and(|(tagged, _)| declared <= tagged)
        })
    } else if resolved.publication.publishes_to_registry() {
        declared
            .as_ref()
            .is_some_and(|declared| published.contains(declared))
    } else {
        false
    };
    let latest_tag = latest.map(|(_, tag)| tag.name);
    // For a maintainer-owned module the maintainer creates the release tag
    // before Toven publishes against it, so surface whether the tag for the
    // declared version is already present — a fail-closed readiness signal.
    // A never-versioned module has no declared version to tag, so it reports
    // `Some(false)` (not ready). A Toven-owned module creates its own tag, so
    // the question does not apply (`None`).
    let maintainer_tag_present = if resolved.entrypoint.is_maintainer_owned() {
        Some(declared.as_ref().is_some_and(|declared| {
            let expected = tag::format(&scheme, declared);
            inputs
                .tags_by_member
                .get(&module.member)
                .is_some_and(|tags| tags.iter().any(|tag| tag.name == expected))
        }))
    } else {
        None
    };
    Ok(ReleaseModuleStatus {
        module: module.key.clone(),
        publication: resolved.publication.clone(),
        is_published,
        declared_version: declared,
        latest_tag,
        host_forge: resolved.host.forge.clone(),
        published_versions: published,
        entrypoint: resolved.entrypoint,
        maintainer_tag_present,
    })
}

/// The `release status` per-unit operation on the shared runtime engine.
///
/// GATHER (the release targets, settings, and tag snapshot) is resolved once
/// into [`StatusInputs`]; each unit streams one module's `module_status`
/// verdict. The per-module registry lookup is a synchronous port call, so it
/// runs on a blocking thread ([`tokio::task::spawn_blocking`]) to let the async
/// engine schedule the modules bounded-parallel.
pub struct StatusOperation {
    inputs: Arc<StatusInputs>,
}

impl StatusOperation {
    /// Wrap gathered inputs as a runnable operation.
    #[must_use]
    pub fn new(inputs: StatusInputs) -> Self {
        Self {
            inputs: Arc::new(inputs),
        }
    }
}

#[async_trait]
impl UnitOperation for StatusOperation {
    type Shared = Arc<StatusInputs>;
    type Outcome = ReleaseModuleStatus;

    async fn gather(&self) -> AppResult<Self::Shared> {
        Ok(Arc::clone(&self.inputs))
    }

    async fn run(
        &self,
        shared: &Self::Shared,
        unit_id: &str,
        _cancel: CancellationToken,
    ) -> AppResult<Completed<Self::Outcome>> {
        let shared = Arc::clone(shared);
        let id = unit_id.to_string();
        let status = tokio::task::spawn_blocking(move || {
            let module = shared.module(&id).ok_or_else(|| {
                AppError::new(ErrorCode::Internal, format!("unknown status unit '{id}'"))
            })?;
            module_status(&shared, module)
        })
        .await
        .map_err(AppError::internal)??;
        Ok(Completed::succeeded(status))
    }
}

/// Build the `release status` operation and its engine unit graph.
///
/// The single entry the CLI drives on [`toven_runtime::execute`]: GATHER runs
/// here (once), and the returned units feed the engine's per-module streaming.
///
/// # Errors
/// Propagates GATHER failures (configuration/discovery/graph, VCS tag listing).
pub fn status_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<(StatusOperation, Vec<UnitSpec>)> {
    let inputs = StatusInputs::gather(request, document, providers, readers, reporter)?;
    let units = inputs.units();
    Ok((StatusOperation::new(inputs), units))
}

/// Project the declared/published/tagged state of every releasable module as a
/// buffered aggregate.
///
/// Retained for programmatic callers; assembled from the same per-unit
/// `module_status` compute the streaming [`StatusOperation`] drives, so the
/// two never diverge. The CLI streams via the engine instead of calling this.
///
/// # Errors
/// Propagates GATHER failures and release-target version I/O failures.
pub fn release_status(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<ReleaseStatus> {
    let inputs = StatusInputs::gather(request, document, providers, readers, reporter)?;
    let mut modules = inputs
        .modules
        .iter()
        .map(|module| module_status(&inputs, module))
        .collect::<AppResult<Vec<_>>>()?;
    modules.sort_by(|left, right| left.module.cmp(&right.module));
    Ok(ReleaseStatus::new(modules))
}

/// List every member repo's tags once, keyed by member.
///
/// Mirrors change detection: each member's VCS adapter enumerates all tags and
/// the per-module baseline resolves against that shared snapshot, so the tag
/// set is fetched once per member rather than once per module.
fn list_member_tags(
    readers: &MemberVcsReaders<'_>,
) -> AppResult<BTreeMap<Option<MemberId>, Vec<TagRef>>> {
    let mut tags = BTreeMap::new();
    for reader in readers.entries() {
        tags.insert(reader.member().cloned(), reader.reader().list_tags(None)?);
    }
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, CommonEcosystemConfig, DiscoverResponse, Oid, Provider, ReleaseConfig,
        TagRef, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::{release_status, status_operation};
    use crate::ReleaseModuleStatus;
    use rskit_errors::AppResult;
    use tokio_util::sync::CancellationToken;
    use toven_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_core::federation::baseline::MemberVcsReaders;
    use toven_core::plan::{PlanRequest, Selection};
    use toven_runtime::UnitStatus;

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(eid("rust"), name).unwrap()
    }

    fn module(name: &str) -> Module {
        Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap())
    }

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid("rust"), RawValue::from(json!({ "release": {} })));
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems,
            modules: std::collections::BTreeMap::new(),
            members: Vec::new(),
            hooks: std::collections::BTreeMap::new(),
            units: std::collections::BTreeMap::new(),
        }
    }

    fn request() -> PlanRequest {
        PlanRequest::new(
            "r1",
            "t",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
        .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))))
    }

    #[test]
    fn status_reports_declared_published_and_tag_per_module() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core.clone()];

        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 2, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs =
            FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("cafe"))]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        assert_eq!(status.modules.len(), 1);
        let entry = &status.modules[0];
        assert_eq!(entry.module, core.key());
        assert_eq!(
            entry.publication,
            toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into()
            }
        );
        assert_eq!(entry.declared_version, Some(Version::new(0, 2, 0)));
        assert_eq!(entry.latest_tag.as_deref(), Some("rust/core@0.1.0"));
        assert_eq!(entry.published_versions, vec![Version::new(0, 1, 0)]);
        assert!(!entry.is_published);
    }

    #[test]
    fn status_reports_a_never_versioned_module_as_unreleased() {
        // A never-tagged tag-only module has no declared version; the
        // pre-release status step must report it as unreleased rather than
        // failing the whole verb.
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core.clone()];
        let target = FakeReleaseTarget::new().with_no_declared_version();
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status = release_status(&request(), &document(), &providers, &readers, &mut reporter)
            .expect("a versionless module is reported, not an error");

        let entry = &status.modules[0];
        assert_eq!(entry.module, core.key());
        assert_eq!(entry.declared_version, None);
        assert_eq!(entry.latest_tag, None);
        assert!(!entry.is_published);
    }

    #[test]
    fn tag_only_status_never_queries_the_registry() {
        // A tag-only module never publishes, so status must not query the
        // registry for it even online — the lookup would hit the network
        // (e.g. `cargo search`) for a set that is unused.
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 2, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(target.clone());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert_eq!(entry.publication, toven_ports::PublicationPolicy::TagOnly);
        assert!(entry.published_versions.is_empty());
        assert!(!entry.is_published);
        assert!(
            !target
                .calls()
                .iter()
                .any(|call| matches!(call, toven_testkit::ReleaseCall::PublishedVersions(_))),
            "tag-only status must not query the registry: {:?}",
            target.calls()
        );
    }

    #[test]
    fn maintainer_owned_status_flags_the_declared_version_tag_as_present() {
        // A maintainer-owned module surfaces whether the maintainer's release
        // tag for the declared version is present — the tag `rust/core@0.1.0`
        // exists, so the flow is ready.
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 1, 0));
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                entrypoint: Some(toven_model::Entrypoint::Maintainer),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs =
            FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("cafe"))]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert!(entry.entrypoint.is_maintainer_owned());
        assert_eq!(entry.maintainer_tag_present, Some(true));
    }

    #[test]
    fn maintainer_owned_status_flags_a_missing_declared_version_tag() {
        // No tag exists for the declared version: a fail-closed preview that the
        // maintainer-owned flow is not yet ready to publish.
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 1, 0));
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                entrypoint: Some(toven_model::Entrypoint::Maintainer),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert!(entry.entrypoint.is_maintainer_owned());
        assert_eq!(entry.maintainer_tag_present, Some(false));
    }

    #[test]
    fn toven_owned_status_leaves_the_maintainer_tag_flag_unset() {
        // A Toven-owned module creates its own tag, so the maintainer-tag
        // readiness question does not apply.
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 1, 0));
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert!(entry.entrypoint.is_toven_owned());
        assert_eq!(entry.maintainer_tag_present, None);
    }

    #[test]
    fn offline_status_never_queries_the_registry() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 2, 0));
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                offline: Some(true),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target.clone());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert!(entry.published_versions.is_empty());
        assert!(!entry.is_published);
        assert!(
            !target
                .calls()
                .iter()
                .any(|call| matches!(call, toven_testkit::ReleaseCall::PublishedVersions(_))),
            "offline status must not query the registry: {:?}",
            target.calls()
        );
    }

    #[test]
    fn offline_status_anchors_is_published_on_the_release_tag() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 2, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                offline: Some(true),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target.clone());
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new().with_tags(vec![
            TagRef::new("rust/core@0.1.0", Oid::new("beef")),
            TagRef::new("rust/core@0.2.0", Oid::new("cafe")),
        ]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert!(entry.published_versions.is_empty());
        assert_eq!(entry.latest_tag.as_deref(), Some("rust/core@0.2.0"));
        assert!(entry.is_published);
        assert!(
            !target
                .calls()
                .iter()
                .any(|call| matches!(call, toven_testkit::ReleaseCall::PublishedVersions(_))),
            "offline status must not query the registry: {:?}",
            target.calls()
        );
    }

    #[test]
    fn offline_status_reports_unpublished_when_tags_lag_the_manifest() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 3, 0));
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                offline: Some(true),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs =
            FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.2.0", Oid::new("cafe"))]);
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert_eq!(entry.latest_tag.as_deref(), Some("rust/core@0.2.0"));
        assert!(!entry.is_published);
    }

    #[test]
    fn status_marks_a_declared_version_already_on_the_registry() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 1, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        let entry = &status.modules[0];
        assert!(entry.is_published);
        assert_eq!(
            entry.publication,
            toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into()
            }
        );
        assert_eq!(entry.latest_tag, None);
    }

    #[test]
    fn status_reports_the_host_forge_a_module_participates_in() {
        // A hosting module surfaces its forge; a module with no host block
        // reports no host participation.
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 1, 0));
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                offline: Some(true),
                host: Some(toven_ports::HostConfig {
                    forge: Some("github".into()),
                    ..toven_ports::HostConfig::default()
                }),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        assert_eq!(status.modules[0].host_forge.as_deref(), Some("github"));
    }

    #[test]
    fn status_reports_no_host_forge_for_a_pure_library() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];

        let target = FakeReleaseTarget::new().with_declared_version(Version::new(0, 1, 0));
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                offline: Some(true),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let vcs = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let status =
            release_status(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        assert_eq!(status.modules[0].host_forge, None);
    }

    /// Records the streamed per-unit lifecycle in arrival order.
    #[derive(Default)]
    struct Recorder {
        started: Vec<String>,
        settled: Vec<(
            String,
            toven_runtime::UnitStatus,
            Option<ReleaseModuleStatus>,
        )>,
    }

    impl toven_runtime::Progress<ReleaseModuleStatus> for Recorder {
        fn started(&mut self, unit_id: &str) -> AppResult<()> {
            self.started.push(unit_id.to_string());
            Ok(())
        }

        fn settled(
            &mut self,
            report: &toven_runtime::UnitReport<ReleaseModuleStatus>,
        ) -> AppResult<()> {
            self.settled.push((
                report.unit_id.clone(),
                report.status,
                report.outcome.clone(),
            ));
            Ok(())
        }
    }

    fn two_module_providers() -> (FakeProvider, FakeVcsReader) {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core"), module("cli")];
        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 2, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]);
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let vcs =
            FakeVcsReader::new().with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("cafe"))]);
        (provider, vcs)
    }

    #[test]
    fn status_units_are_edgeless_and_independent() {
        // Status modules share nothing, so every unit is edgeless — the engine
        // schedules them as one bounded-parallel wave.
        let (provider, vcs) = two_module_providers();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (_op, units) =
            status_operation(&request(), &document(), &providers, &readers, &mut reporter).unwrap();

        assert_eq!(units.len(), 2);
        assert!(
            units.iter().all(|unit| unit.depends_on.is_empty()),
            "status units must be edgeless: {units:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_streams_a_settled_outcome_per_releasable_module() {
        // The engine streams a start + settled pair per module, each settled
        // event carrying that module's typed status outcome — never buffering a
        // terminal aggregate before first output.
        let (provider, vcs) = two_module_providers();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (op, units) =
            status_operation(&request(), &document(), &providers, &readers, &mut reporter).unwrap();
        let mut rec = Recorder::default();
        let summary = toven_runtime::execute(
            &units,
            op,
            toven_runtime::EngineConfig {
                jobs: 2,
                fail_fast: false,
            },
            &mut rec,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 2);
        assert!(!summary.has_failures());
        assert_eq!(rec.started.len(), 2);
        assert_eq!(rec.settled.len(), 2);
        for (unit_id, status, outcome) in &rec.settled {
            assert_eq!(*status, UnitStatus::Succeeded);
            let outcome = outcome.as_ref().expect("settled status carries an outcome");
            assert_eq!(&outcome.module.to_string(), unit_id);
            assert_eq!(outcome.declared_version, Some(Version::new(0, 2, 0)));
            assert_eq!(outcome.published_versions, vec![Version::new(0, 1, 0)]);
        }
    }
}
