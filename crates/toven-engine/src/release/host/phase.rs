//! The hosted-release phase: cut a forge Release for every tagged module after
//! tag and registry publish, target-agnostic over the one topological order.
//!
//! The phase is engine-owned and config-gated: only modules whose resolved
//! `[…release].host` names a `forge` participate. It resolves each Release's
//! tag (via the module's target tag scheme), note body (changelog or override),
//! draft/prerelease flags, and asset set, then hands a fully-resolved
//! [`HostedRelease`] to the forge adapter. `--dry-run` rehearses this without
//! invoking any forge (see [`rehearse`](super::rehearse)); `--no-push` skips the
//! phase entirely, consistent with tag push.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{MemberId, Module, ModuleKey};
use toven_ports::{HostedRelease, ReleaseAsset, ReleaseHost};

use super::github::GithubReleaseHost;
use crate::federation::release::MemberReleaseRepos;
use crate::release::settings::ResolvedReleaseSettings;
use crate::release::{ReleasePlan, ReleaseStats, ReleaseTargets, tag};

/// Forge identifier for the GitHub hosted-release adapter.
const FORGE_GITHUB: &str = "github";

/// Concrete forge hosts resolved from config, keyed by forge identifier.
#[allow(clippy::redundant_pub_crate)]
pub(crate) type ReleaseHosts = BTreeMap<String, Box<dyn ReleaseHost>>;

/// One resolved hosted Release plus the forge that will cut it.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct PlannedHostRelease {
    /// Forge identifier the Release is cut on.
    pub forge: String,
    /// Member repo whose pushed tag this Release is cut against; `None` for the
    /// degenerate single-repo project.
    pub member: Option<MemberId>,
    /// Fully-resolved hosted Release.
    pub release: HostedRelease,
}

/// Build the concrete forge hosts referenced by the resolved release settings.
///
/// # Errors
/// Returns a typed error for an unsupported forge identifier.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn build_hosts(
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<ReleaseHosts> {
    let mut hosts = ReleaseHosts::new();
    for forge in settings
        .values()
        .filter_map(|entry| entry.host.forge.as_deref())
    {
        if hosts.contains_key(forge) {
            continue;
        }
        hosts.insert(forge.to_string(), build_host(forge)?);
    }
    Ok(hosts)
}

fn build_host(forge: &str) -> AppResult<Box<dyn ReleaseHost>> {
    match forge {
        FORGE_GITHUB => Ok(Box::new(GithubReleaseHost::new())),
        other => Err(AppError::invalid_input(
            "release.host.forge",
            format!(
                "unsupported forge '{other}'; only 'github' is supported (gitlab is a documented seam)"
            ),
        )),
    }
}

/// Resolve the hosted Releases a real run would cut, in the plan's publish order.
///
/// Only entries with a planned version whose settings name a forge participate.
///
/// # Errors
/// Propagates a module/target lookup failure or a tag-scheme construction error.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn planned_host_releases(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    project_root: &std::path::Path,
) -> AppResult<Vec<PlannedHostRelease>> {
    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();

    let mut planned = Vec::new();
    for entry in &plan.entries {
        let Some(version) = &entry.planned_version else {
            continue;
        };
        let Some(host) = settings.get(&entry.module).map(|resolved| &resolved.host) else {
            continue;
        };
        let Some(forge) = &host.forge else {
            continue;
        };

        let module = module_by_ref.get(&entry.module).copied().ok_or_else(|| {
            AppError::invalid_input(
                "release.modules",
                format!("unknown module '{}'", entry.module),
            )
        })?;
        let target = targets
            .get(&(module.member.clone(), module.id.ecosystem.clone()))
            .map(Box::as_ref)
            .ok_or_else(|| {
                AppError::invalid_input(
                    "release.target",
                    format!("module '{}' has no release target", module.key()),
                )
            })?;
        let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
        let tag = tag::format(&scheme, version);

        let notes = host
            .notes
            .clone()
            .unwrap_or_else(|| changelog_notes(&entry.changelog));
        let prerelease = host
            .prerelease
            .unwrap_or_else(|| entry.prerelease_channel.is_some());
        let assets = host
            .assets
            .iter()
            .map(|path| ReleaseAsset::new(project_root.join(path)))
            .collect();

        let release = HostedRelease::new(tag.clone(), tag, notes)
            .with_draft(host.draft)
            .with_prerelease(prerelease)
            .with_assets(assets);
        planned.push(PlannedHostRelease {
            forge: forge.clone(),
            member: module.member.clone(),
            release,
        });
    }
    Ok(planned)
}

/// Cut every planned hosted Release through its forge host, accounting outcomes.
///
/// Each Release is cut from its member repo's root so a forge command targets the
/// repository whose tags that member pushed; the degenerate single-repo project
/// (and any member without a resolved repo) falls back to `project_root`.
///
/// # Errors
/// Returns a typed error when a forge host is missing or a forge Release fails.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn run_host_phase(
    planned: &[PlannedHostRelease],
    hosts: &ReleaseHosts,
    repos: &MemberReleaseRepos<'_>,
    project_root: &std::path::Path,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    for entry in planned {
        let host = hosts.get(&entry.forge).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("no host adapter resolved for forge '{}'", entry.forge),
            )
        })?;
        let root = repos.root_for(entry.member.as_ref()).unwrap_or(project_root);
        host.ensure_release(root, &entry.release)?;
        stats.hosted_releases += 1;
    }
    Ok(())
}

/// Derive the release-note body from a module's changelog entry: the detailed
/// lines when present, otherwise the short summary.
fn changelog_notes(changelog: &crate::release::ChangelogEntry) -> String {
    if changelog.lines.is_empty() {
        changelog.summary.clone()
    } else {
        changelog.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{BumpLevel, HostConfig, HostReleaseOutcome, ReleaseConfig, ReleaseMutation};
    use toven_testkit::{FakeReleaseHost, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter};

    use super::{build_hosts, planned_host_releases, run_host_phase};
    use crate::federation::release::{MemberReleaseRepo, MemberReleaseRepos};
    use crate::release::ResolvedReleaseSettings;
    use crate::release::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseEntry, ReleasePlan,
        ReleaseStats, ReleaseTargets,
    };

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

    fn entry(name: &str, prerelease: Option<&str>) -> ReleaseEntry {
        let version = Version::new(0, 1, 1);
        ReleaseEntry {
            module: mkey(name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(version.clone()),
            level: BumpLevel::Patch,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: prerelease.map(str::to_string),
            up_to_date: false,
            mutation: ReleaseMutation::version(version),
            publish_needed: true,
            tag_format: None,
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(
                mkey(name),
                "changed core",
                vec!["- did a thing".into()],
            ),
        }
    }

    fn targets() -> ReleaseTargets {
        let mut map = ReleaseTargets::new();
        map.insert((None, eid()), Box::new(FakeReleaseTarget::new()));
        map
    }

    fn settings(
        name: &str,
        host: Option<HostConfig>,
    ) -> BTreeMap<ModuleKey, ResolvedReleaseSettings> {
        let config = ReleaseConfig {
            host,
            ..ReleaseConfig::default()
        };
        let resolved = ResolvedReleaseSettings::resolve(&config, None).unwrap();
        let mut map = BTreeMap::new();
        map.insert(mkey(name), resolved);
        map
    }

    fn github_host() -> HostConfig {
        HostConfig {
            forge: Some("github".into()),
            ..HostConfig::default()
        }
    }

    #[test]
    fn planned_releases_only_include_modules_with_a_configured_forge() {
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry("core", None)]);
        let modules = vec![module("core")];

        // No host configured: nothing to cut.
        let none = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", None),
            Path::new("/repo"),
        )
        .unwrap();
        assert!(none.is_empty());

        // Host configured: one release, tag from the target scheme.
        let planned = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", Some(github_host())),
            Path::new("/repo"),
        )
        .unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].forge, "github");
        assert_eq!(planned[0].release.tag, "rust/core@0.1.1");
        // Notes default to the changelog body.
        assert_eq!(planned[0].release.notes, "- did a thing");
    }

    #[test]
    fn prerelease_flag_derives_from_channel_when_unset() {
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry("core", Some("rc"))]);
        let modules = vec![module("core")];

        let planned = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", Some(github_host())),
            Path::new("/repo"),
        )
        .unwrap();

        assert!(planned[0].release.prerelease);
    }

    #[test]
    fn host_config_overrides_flags_notes_and_assets() {
        let host = HostConfig {
            forge: Some("github".into()),
            draft: Some(true),
            prerelease: Some(false),
            notes: Some("handcrafted".into()),
            assets: Some(vec!["target/toven/release/core.cdx.json".into()]),
        };
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry("core", Some("rc"))]);
        let modules = vec![module("core")];

        let planned = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", Some(host)),
            Path::new("/repo"),
        )
        .unwrap();

        let release = &planned[0].release;
        assert!(release.draft);
        // Explicit config wins over the derived prerelease channel.
        assert!(!release.prerelease);
        assert_eq!(release.notes, "handcrafted");
        assert_eq!(release.assets.len(), 1);
        // Assets resolve against the project root.
        assert!(
            release.assets[0]
                .path
                .ends_with("target/toven/release/core.cdx.json")
        );
        assert!(release.assets[0].path.starts_with("/repo"));
    }

    #[test]
    fn run_host_phase_cuts_each_release_from_the_member_repo_root() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", None), entry("api", None)],
        );
        let modules = vec![module("core"), module("api")];
        let mut all = settings("core", Some(github_host()));
        all.extend(settings("api", Some(github_host())));
        let planned =
            planned_host_releases(&plan, &modules, &targets(), &all, Path::new("/repo")).unwrap();

        let host = FakeReleaseHost::new().with_outcome(HostReleaseOutcome::Updated);
        let mut hosts = super::ReleaseHosts::new();
        hosts.insert("github".to_string(), Box::new(host.clone()));
        let mut stats = ReleaseStats::new(2);

        // The degenerate project's member repo root wins over the project-root
        // fallback: `gh` runs from the repo whose tags this member pushed.
        let reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            PathBuf::from("/member/repo"),
            &reader,
            &writer,
        )]);
        run_host_phase(&planned, &hosts, &repos, Path::new("/fallback"), &mut stats).unwrap();

        assert_eq!(stats.hosted_releases, 2);
        let calls = host.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].release.tag, "rust/core@0.1.1");
        assert_eq!(calls[0].root, Path::new("/member/repo"));
    }

    #[test]
    fn run_host_phase_falls_back_to_project_root_for_an_unknown_member() {
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry("core", None)]);
        let modules = vec![module("core")];
        let planned = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", Some(github_host())),
            Path::new("/repo"),
        )
        .unwrap();

        let host = FakeReleaseHost::new().with_outcome(HostReleaseOutcome::Updated);
        let mut hosts = super::ReleaseHosts::new();
        hosts.insert("github".to_string(), Box::new(host.clone()));
        let mut stats = ReleaseStats::new(1);

        // No member repo resolved: the project root is the cwd.
        let repos = MemberReleaseRepos::new(Vec::new());
        run_host_phase(&planned, &hosts, &repos, Path::new("/fallback"), &mut stats).unwrap();

        assert_eq!(host.calls()[0].root, Path::new("/fallback"));
    }

    #[test]
    fn build_hosts_rejects_an_unsupported_forge() {
        let host = HostConfig {
            forge: Some("bitbucket".into()),
            ..HostConfig::default()
        };
        let Err(error) = build_hosts(&settings("core", Some(host))) else {
            panic!("unsupported forge must be rejected");
        };
        assert!(error.to_string().contains("bitbucket"), "{error}");
    }

    #[test]
    fn build_hosts_resolves_github() {
        let hosts = build_hosts(&settings("core", Some(github_host()))).unwrap();
        assert!(hosts.contains_key("github"));
    }
}
