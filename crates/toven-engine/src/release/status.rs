//! Read-only release status projection.
//!
//! Reports, per releasable module, the version its manifest declares, the
//! newest release tag cut for it, and the versions the registry already reports
//! as published — all without mutating any manifest, tag, or registry.

use std::collections::BTreeMap;

use rskit_errors::AppResult;
use toven_model::MemberId;
use toven_ports::{Provider, Reporter, TagRef};

use super::{ReleaseModuleStatus, ReleaseStatus, tag};
use toven_engine_core::config::Document;
use toven_engine_core::federation::baseline::MemberVcsReaders;
use toven_engine_core::federation::resolve::PathDriverLocator;
use toven_engine_core::plan::{PlanRequest, prepare_front};

use super::plan::{release_targets, resolve_release_settings};

/// Project the declared/published/tagged state of every releasable module.
///
/// A module is releasable when its ecosystem adapter exposes a release target;
/// modules without one are omitted. Registry lookups are best-effort per the
/// [`VersionSource`](toven_ports::VersionSource) contract, so a partial
/// published set still yields a status.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, VCS tag-listing failures,
/// and release-target version I/O failures.
pub fn release_status(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<ReleaseStatus> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let settings = resolve_release_settings(&context, &targets)?;

    let tags_by_member = list_member_tags(readers)?;
    let mut modules = Vec::new();
    for module in &context.federation.modules {
        let key = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&key) else {
            continue;
        };
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        if !resolved.publication.releases() {
            continue;
        }
        let declared = target.declared_version(module)?;
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
            target.published_versions(module)?
        };
        let scheme = target.tag_scheme(module, resolved.tag_format.as_deref())?;
        let latest = tags_by_member
            .get(&module.member)
            .and_then(|tags| tag::latest(&scheme, tags));
        // Offline there is no registry set to consult — idempotency anchors on
        // release tags instead (mirroring plan-time `planned <= tagged`), so the
        // published verdict comes from the newest release tag: a declared
        // version at/below it has already been released.
        let is_published = if resolved.offline {
            latest
                .as_ref()
                .is_some_and(|(tagged, _)| &declared <= tagged)
        } else if resolved.publication.publishes_to_registry() {
            published.contains(&declared)
        } else {
            false
        };
        let latest_tag = latest.map(|(_, tag)| tag.name);
        modules.push(ReleaseModuleStatus {
            module: module.key(),
            publication: resolved.publication.clone(),
            is_published,
            declared_version: declared,
            latest_tag,
            host_forge: resolved.host.forge.clone(),
            published_versions: published,
        });
    }
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

    use super::release_status;
    use toven_engine_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_engine_core::federation::baseline::MemberVcsReaders;
    use toven_engine_core::plan::{PlanRequest, Selection};

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
        assert_eq!(entry.declared_version, Version::new(0, 2, 0));
        assert_eq!(entry.latest_tag.as_deref(), Some("rust/core@0.1.0"));
        assert_eq!(entry.published_versions, vec![Version::new(0, 1, 0)]);
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
}
