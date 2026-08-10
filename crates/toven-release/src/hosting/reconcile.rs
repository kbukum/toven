//! Reconcile a published-but-unhosted release: complete the hosted forge
//! Release for the current published version without bumping, tagging, or
//! re-publishing.
//!
//! A release run publishes the git tag and the registry version, then cuts the
//! hosted Release last. If it fails after publishing but before the hosted
//! Release, the tag and registry version are live and immutable, yet no forge
//! Release exists. The immutable state cannot be re-published, and the
//! automatic `release publish` re-dispatch never re-plans an existing tag (a
//! changed module always plans a forward *bump*, an unchanged module is dropped
//! from the plan entirely), so the normal bump planner can never complete that
//! missing Release.
//!
//! This pre-pass closes that gap. It keys off the already-published state — a
//! registry module whose current published version's release tag exists but
//! whose hosted Release is missing — and completes the Release through the
//! forge's own create-or-verify path. It is engine-owned and idempotent:
//! re-running against a complete state creates nothing.

use std::collections::BTreeMap;
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{Module, ModuleKey};
use toven_ports::{HostReleaseOutcome, HostedRelease, PublicationPolicy, ReleaseAsset};

use crate::hosting::host::{PlannedHostRelease, ReleaseHosts};
use crate::model::settings::ResolvedReleaseSettings;
use crate::model::tag;
use crate::versioning::changelog;
use crate::{ReleaseStats, ReleaseTargets};
use toven_core::federation::member_repo::MemberReleaseRepos;

/// Plan the hosted Releases that must be reconciled for the current published
/// state.
///
/// A module is a reconcile candidate when it is registry-published (the
/// incident's shape; tag-only and excluded modules fall through to normal
/// planning), names a hosted forge, is not offline (an offline run never
/// queries the registry, so no published version can be derived), has at least
/// one published version, and that version's release tag already exists in the
/// member repo — the immutable tag an incomplete run pushed. The resulting
/// [`HostedRelease`] mirrors [`planned_host_releases`](super::host) exactly
/// (title = tag, notes from the host override or the changelog summary,
/// draft/prerelease flags, assets) so the reconciled Release is identical to
/// the one the original run would have cut.
///
/// # Errors
/// Propagates a target version/tag-scheme query or a tag listing failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn plan_reconcile_releases(
    modules: &[Module],
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    repos: &MemberReleaseRepos<'_>,
) -> AppResult<Vec<PlannedHostRelease>> {
    let mut planned = Vec::new();
    for module in modules {
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        // An offline run anchors idempotency on tags only and never queries the
        // registry, so it cannot derive a published version to reconcile.
        if resolved.offline {
            continue;
        }
        // Scope to registry-published modules — the incident's shape. Tag-only
        // and excluded modules fall through to the normal release path.
        if !matches!(resolved.publication, PublicationPolicy::Registry { .. }) {
            continue;
        }
        let Some(forge) = resolved.host.forge.clone() else {
            continue;
        };
        let Some(target) = targets
            .get(&(module.member.clone(), module.id.ecosystem.clone()))
            .map(Box::as_ref)
        else {
            continue;
        };

        let published = target.published_versions(module)?;
        let Some(version) = published.iter().max().cloned() else {
            continue;
        };
        let scheme = target.tag_scheme(module, resolved.tag_format.as_deref())?;
        let tag = tag::format(&scheme, &version);

        // The published version's release tag must already exist — the immutable
        // tag the incomplete run pushed — before its hosted Release is completed.
        let Some(reader) = repos.reader_for(module.member.as_ref()) else {
            continue;
        };
        if !reader
            .list_tags(None)?
            .iter()
            .any(|found| found.name == tag)
        {
            continue;
        }

        // A module with a single published version has no prior release to diff
        // against, so its reconciled notes read as an initial release rather than
        // a (record-free) dependency cascade.
        let initial = published.len() <= 1;
        let notes = resolved
            .host
            .notes
            .clone()
            .unwrap_or_else(|| changelog::entry(module, &[], initial).summary);
        let prerelease = resolved
            .host
            .prerelease
            .unwrap_or_else(|| !version.pre.is_empty());
        let assets = resolved
            .host
            .assets
            .iter()
            .map(|path| ReleaseAsset::new(path.clone()))
            .collect();
        let release = HostedRelease::new(tag.clone(), tag, notes)
            .with_draft(resolved.host.draft)
            .with_prerelease(prerelease)
            .with_assets(assets);
        planned.push(PlannedHostRelease {
            forge,
            member: module.member.clone(),
            release,
        });
    }
    Ok(planned)
}

/// Complete every missing hosted Release for the current published state,
/// reporting whether any Release was actually created.
///
/// Each planned Release is cut through the forge's create-or-verify path from
/// its member repo root. The return value is the short-circuit signal: `true`
/// only when at least one Release was freshly [`Created`], meaning the run
/// resumed a genuinely incomplete release and should stop before the bump
/// planner. When every candidate was already complete (nothing missing) it
/// returns `false`, so the caller falls through to a normal release and a
/// legitimate new version is never blocked.
///
/// [`Created`]: HostReleaseOutcome::Created
///
/// # Errors
/// Returns a typed error when a planned forge has no resolved host adapter or a
/// forge Release fails, and propagates planning errors.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn reconcile_hosted_releases(
    modules: &[Module],
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    repos: &MemberReleaseRepos<'_>,
    hosts: &ReleaseHosts,
    project_root: &Path,
    stats: &mut ReleaseStats,
) -> AppResult<bool> {
    let planned = plan_reconcile_releases(modules, targets, settings, repos)?;
    let mut created = false;
    for entry in &planned {
        let host = hosts.get(&entry.forge).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("no host adapter resolved for forge '{}'", entry.forge),
            )
        })?;
        let root = repos
            .root_for(entry.member.as_ref())
            .unwrap_or(project_root);
        // Complete only a genuinely missing Release. An existing Release is left
        // untouched: the immutable verify runs only on the normal publish path,
        // never here, so a Release whose notes legitimately differ from freshly
        // authored ones is not reported as a conflict on every re-dispatch.
        if host.release_exists(root, &entry.release.tag)? {
            continue;
        }
        if matches!(
            host.ensure_release(root, &entry.release)?,
            HostReleaseOutcome::Created
        ) {
            created = true;
            stats.hosted_releases += 1;
        }
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_version::semver::Version;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{
        HostConfig, HostReleaseOutcome, Oid, ReleaseConfig, TagRef, VcsReader, VcsWriter,
    };
    use toven_testkit::{FakeReleaseHost, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter};

    use super::{plan_reconcile_releases, reconcile_hosted_releases};
    use crate::hosting::host::ReleaseHosts;
    use crate::model::settings::ResolvedReleaseSettings;
    use crate::{ReleaseStats, ReleaseTargets};
    use toven_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};

    fn eid() -> EcosystemId {
        EcosystemId::new("rust").unwrap()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(eid(), name).unwrap()
    }

    fn mkey(name: &str) -> ModuleKey {
        ModuleKey::bare(mref(name))
    }

    fn module(name: &str) -> Module {
        Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap())
    }

    fn github_host() -> HostConfig {
        HostConfig {
            forge: Some("github".into()),
            ..HostConfig::default()
        }
    }

    fn settings(
        name: &str,
        config: &ReleaseConfig,
    ) -> BTreeMap<ModuleKey, ResolvedReleaseSettings> {
        let resolved = ResolvedReleaseSettings::resolve(config, None).unwrap();
        let mut map = BTreeMap::new();
        map.insert(mkey(name), resolved);
        map
    }

    fn registry_host_config() -> ReleaseConfig {
        ReleaseConfig {
            registry: Some("crates-io".into()),
            host: Some(github_host()),
            ..ReleaseConfig::default()
        }
    }

    fn targets(published: Vec<Version>) -> ReleaseTargets {
        let mut map = ReleaseTargets::new();
        map.insert(
            (None, eid()),
            Box::new(FakeReleaseTarget::new().with_published_versions(published)),
        );
        map
    }

    fn repos<'a>(reader: &'a dyn VcsReader, writer: &'a dyn VcsWriter) -> MemberReleaseRepos<'a> {
        MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            AbsPath::new("/repo").unwrap().as_path().to_path_buf(),
            reader,
            writer,
        )])
    }

    // A registry module whose current published version's tag exists but whose
    // hosted Release is missing is the reconcile candidate: exactly one release,
    // titled and tagged from the published version.
    #[test]
    fn a_published_and_tagged_registry_module_is_a_reconcile_candidate() {
        let modules = vec![module("core")];
        let targets = targets(vec![Version::new(0, 1, 0)]);
        let settings = settings("core", &registry_host_config());
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("dd4a494"))]);
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);

        let planned = plan_reconcile_releases(&modules, &targets, &settings, &repos).unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].forge, "github");
        assert_eq!(planned[0].release.tag, "rust/core@0.1.0");
        assert_eq!(planned[0].release.title, "rust/core@0.1.0");
    }

    // A published version whose release tag does not exist is not reconciled: the
    // tag is the anchor proving a real, immutable release was left incomplete.
    #[test]
    fn a_published_version_without_its_tag_is_not_reconciled() {
        let modules = vec![module("core")];
        let targets = targets(vec![Version::new(0, 1, 0)]);
        let settings = settings("core", &registry_host_config());
        let reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);

        let planned = plan_reconcile_releases(&modules, &targets, &settings, &repos).unwrap();

        assert!(planned.is_empty());
    }

    // A module with no published version has nothing to reconcile.
    #[test]
    fn a_module_with_no_published_version_is_not_reconciled() {
        let modules = vec![module("core")];
        let targets = targets(Vec::new());
        let settings = settings("core", &registry_host_config());
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("dd4a494"))]);
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);

        let planned = plan_reconcile_releases(&modules, &targets, &settings, &repos).unwrap();

        assert!(planned.is_empty());
    }

    // A module with no configured forge cuts no hosted Release, so there is
    // nothing to reconcile even when published and tagged.
    #[test]
    fn a_module_without_a_forge_is_not_reconciled() {
        let modules = vec![module("core")];
        let targets = targets(vec![Version::new(0, 1, 0)]);
        let settings = settings(
            "core",
            &ReleaseConfig {
                registry: Some("crates-io".into()),
                ..ReleaseConfig::default()
            },
        );
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("dd4a494"))]);
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);

        let planned = plan_reconcile_releases(&modules, &targets, &settings, &repos).unwrap();

        assert!(planned.is_empty());
    }

    // A tag-only module never publishes to a registry, so it is out of the
    // reconcile scope and falls through to the normal release path.
    #[test]
    fn a_tag_only_module_is_not_reconciled() {
        let modules = vec![module("core")];
        let targets = targets(vec![Version::new(0, 1, 0)]);
        let settings = settings(
            "core",
            &ReleaseConfig {
                publish: Some(false),
                host: Some(github_host()),
                ..ReleaseConfig::default()
            },
        );
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("dd4a494"))]);
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);

        let planned = plan_reconcile_releases(&modules, &targets, &settings, &repos).unwrap();

        assert!(planned.is_empty());
    }

    // Reconcile short-circuits (returns true) and accounts the release only when
    // the forge actually created a missing Release.
    #[test]
    fn reconcile_reports_created_and_counts_the_release() {
        let modules = vec![module("core")];
        let targets = targets(vec![Version::new(0, 1, 0)]);
        let settings = settings("core", &registry_host_config());
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("dd4a494"))]);
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);
        let host = FakeReleaseHost::new().with_outcome(HostReleaseOutcome::Created);
        let mut hosts = ReleaseHosts::new();
        hosts.insert("github".to_string(), Box::new(host.clone()));
        let mut stats = ReleaseStats::new(0);

        let created = reconcile_hosted_releases(
            &modules,
            &targets,
            &settings,
            &repos,
            &hosts,
            Path::new("/repo"),
            &mut stats,
        )
        .unwrap();

        assert!(created, "a missing Release was created");
        assert_eq!(stats.hosted_releases, 1);
        assert_eq!(host.calls().len(), 1);
        assert_eq!(host.calls()[0].release.tag, "rust/core@0.1.0");
    }

    // When the Release already exists, reconcile creates nothing and does not
    // short-circuit — the run falls through to the normal release path so a
    // legitimate new version is never blocked, and the immutable verify is never
    // run against the existing Release.
    #[test]
    fn reconcile_does_not_short_circuit_when_already_complete() {
        let modules = vec![module("core")];
        let targets = targets(vec![Version::new(0, 1, 0)]);
        let settings = settings("core", &registry_host_config());
        let reader = FakeVcsReader::new()
            .with_tags(vec![TagRef::new("rust/core@0.1.0", Oid::new("dd4a494"))]);
        let writer = FakeVcsWriter::new();
        let repos = repos(&reader, &writer);
        let host = FakeReleaseHost::new().with_existing(true);
        let mut hosts = ReleaseHosts::new();
        hosts.insert("github".to_string(), Box::new(host.clone()));
        let mut stats = ReleaseStats::new(0);

        let created = reconcile_hosted_releases(
            &modules,
            &targets,
            &settings,
            &repos,
            &hosts,
            Path::new("/repo"),
            &mut stats,
        )
        .unwrap();

        assert!(!created, "nothing missing, so no short-circuit");
        assert_eq!(stats.hosted_releases, 0);
        assert!(
            host.calls().is_empty(),
            "an existing Release is never verified or clobbered by reconcile"
        );
    }
}
