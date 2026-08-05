//! The hosted-release phase: cut a forge Release for every tagged module after
//! tag and registry publish, target-agnostic over the one topological order.
//!
//! The phase is engine-owned and config-gated: only modules whose resolved
//! `[…release].host` names a `forge` participate. It resolves each Release's
//! tag (via the module's target tag scheme), note body (changelog or override),
//! draft/prerelease flags, and asset set, then hands a fully-resolved
//! [`HostedRelease`] to the forge adapter. `--dry-run` rehearses this without
//! invoking any forge (see [`rehearse`](super::rehearse)); `--no-push` skips
//! the phase entirely, consistent with tag push.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{MemberId, Module, ModuleKey};
use toven_ports::{HostedRelease, ReleaseAsset, ReleaseHost};

use super::github::GithubReleaseHost;
use super::gitlab::GitlabReleaseHost;
use crate::federation::release::MemberReleaseRepos;
use crate::release::settings::ResolvedReleaseSettings;
use crate::release::{ReleasePlan, ReleaseStats, ReleaseTargets, tag};

/// Forge identifier for the GitHub hosted-release adapter.
const FORGE_GITHUB: &str = "github";

/// Forge identifier for the GitLab hosted-release adapter.
const FORGE_GITLAB: &str = "gitlab";

/// Concrete forge hosts resolved from config, keyed by forge identifier.
#[allow(clippy::redundant_pub_crate)]
pub(crate) type ReleaseHosts = BTreeMap<String, Box<dyn ReleaseHost>>;

/// One resolved hosted Release plus the forge that will cut it.
#[allow(clippy::redundant_pub_crate)]
#[derive(Debug)]
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
        FORGE_GITLAB => Ok(Box::new(GitlabReleaseHost::new())),
        other => Err(AppError::invalid_input(
            "release.host.forge",
            format!("unsupported forge '{other}'; supported forges are 'github' and 'gitlab'"),
        )),
    }
}

/// Resolve the hosted Releases a real run would cut, in the plan's publish
/// order.
///
/// Only entries with a planned version whose settings name a forge participate.
///
/// # Errors
/// Propagates a module/target lookup failure or a tag-scheme construction
/// error, and reports an internal error if a planned module has no resolved
/// release settings (the plan and settings are derived from the same context).
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn planned_host_releases(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
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
            return Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "no resolved release settings for planned module '{}'",
                    entry.module
                ),
            ));
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
        // A version carrying a prerelease identifier (`0.1.0-alpha.1`) is a
        // prerelease whether it came from an explicit `--pre` channel or from the
        // version the module already declares, as a first release does.
        let prerelease = host
            .prerelease
            .unwrap_or_else(|| entry.prerelease_channel.is_some() || !version.pre.is_empty());
        let assets = host
            .assets
            .iter()
            .map(|path| ReleaseAsset::new(path.clone()))
            .collect();

        let release = HostedRelease::new(tag.clone(), tag, notes)
            .with_draft(host.draft)
            .with_prerelease(prerelease)
            .with_assets(assets);
        merge_planned(
            &mut planned,
            PlannedHostRelease {
                forge: forge.clone(),
                member: module.member.clone(),
                release,
            },
            &entry.module,
        )?;
    }
    Ok(planned)
}

/// Fold `candidate` into `planned`, collapsing modules that resolve to the same
/// hosted Release.
///
/// A `tag_format` that omits the module (e.g. `v{version}` for a
/// single-version workspace) maps every module onto one tag, which is one
/// hosted Release — not one per module. Assets and notes from each contributing
/// module are unioned deterministically; conflicting `draft`/`prerelease` flags,
/// or an asset path contributed with divergent labels, are a typed configuration
/// error rather than a last-writer-wins surprise.
fn merge_planned(
    planned: &mut Vec<PlannedHostRelease>,
    candidate: PlannedHostRelease,
    module: &ModuleKey,
) -> AppResult<()> {
    let Some(existing) = planned.iter_mut().find(|entry| {
        entry.forge == candidate.forge
            && entry.member == candidate.member
            && entry.release.tag == candidate.release.tag
    }) else {
        planned.push(candidate);
        return Ok(());
    };

    if existing.release.draft != candidate.release.draft
        || existing.release.prerelease != candidate.release.prerelease
    {
        return Err(AppError::invalid_input(
            "release.host",
            format!(
                "modules sharing release tag '{}' disagree on the hosted Release flags; module \
                 '{module}' resolves draft={}/prerelease={} against draft={}/prerelease={}",
                candidate.release.tag,
                candidate.release.draft,
                candidate.release.prerelease,
                existing.release.draft,
                existing.release.prerelease,
            ),
        ));
    }

    for asset in candidate.release.assets {
        if let Some(present) = existing
            .release
            .assets
            .iter()
            .find(|present| present.path == asset.path)
        {
            if present.label != asset.label {
                return Err(AppError::invalid_input(
                    "release.host",
                    format!(
                        "modules sharing release tag '{}' disagree on the label for asset '{}'; \
                         module '{module}' resolves {:?} against {:?}",
                        candidate.release.tag,
                        asset.path.display(),
                        asset.label,
                        present.label,
                    ),
                ));
            }
            continue;
        }
        existing.release.assets.push(asset);
    }
    if !candidate.release.notes.is_empty() {
        existing.release.notes = crate::release::changelog::merge_notes(
            &existing.release.notes,
            &candidate.release.notes,
        );
    }
    Ok(())
}

/// Cut every planned hosted Release through its forge host, accounting
/// outcomes.
///
/// Each Release is cut from its member repo's root so a forge command targets
/// the repository whose tags that member pushed; the degenerate single-repo
/// project (and any member without a resolved repo) falls back to
/// `project_root`.
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
        let root = repos
            .root_for(entry.member.as_ref())
            .unwrap_or(project_root);
        host.ensure_release(root, &entry.release)?;
        stats.hosted_releases += 1;
    }
    Ok(())
}

/// The hosted-release body for a module: its grouped, attributed changelog
/// lines.
///
/// The plan `summary` (`3 commits`, `dependency cascade`, `initial release`) is
/// a table cell for the release *plan*, never release-body prose — a module
/// with no commits in range contributes an empty body and is folded away when
/// its Release is merged with a sibling that does carry notes.
fn changelog_notes(changelog: &crate::release::ChangelogEntry) -> String {
    changelog.lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use rskit_errors::ErrorCode;
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{
        BumpLevel, HostConfig, HostReleaseOutcome, HostedRelease, ReleaseAsset, ReleaseConfig,
        ReleaseMutation,
    };
    use toven_testkit::{FakeReleaseHost, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter};

    use super::{
        PlannedHostRelease, build_hosts, merge_planned, planned_host_releases, run_host_phase,
    };
    use crate::federation::release::{MemberReleaseRepo, MemberReleaseRepos};
    use crate::release::ResolvedReleaseSettings;
    use crate::release::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseEntry, ReleasePlan,
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
            planned_tag: Some(format!("rust/{name}@{version}")),
            level: BumpLevel::Patch,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: prerelease.map(str::to_string),
            up_to_date: false,
            mutation: ReleaseMutation::version(version),
            publication: toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into(),
            },
            publish_needed: true,
            tag_format: None,
            tag_message: None,
            signer: None,
            commit_message: None,
            token_env: None,
            visibility: toven_ports::Visibility::Public,
            push: PushPolicy::BranchAndTags,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(
                mkey(name),
                "changed core",
                vec!["- did a thing".into()],
            ),
            changelog_path: "CHANGELOG.md".into(),
            changelog_roll: false,
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
        let none =
            planned_host_releases(&plan, &modules, &targets(), &settings("core", None)).unwrap();
        assert!(none.is_empty());

        // Host configured: one release, tag from the target scheme.
        let planned = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", Some(github_host())),
        )
        .unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].forge, "github");
        assert_eq!(planned[0].release.tag, "rust/core@0.1.1");
        // Notes default to the changelog body.
        assert_eq!(planned[0].release.notes, "- did a thing");
    }

    #[test]
    fn modules_sharing_one_release_tag_collapse_into_a_single_hosted_release() {
        // `v{version}` omits the module, so a single-version workspace maps every
        // module onto one tag — and therefore onto one hosted Release.
        let shared_format = |name: &str| {
            let mut entry = entry(name, None);
            entry.tag_format = Some("v{version}".into());
            entry
        };
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![shared_format("core"), shared_format("app")],
        );
        let modules = vec![module("core"), module("app")];
        let host = HostConfig {
            forge: Some("github".into()),
            assets: Some(vec!["dist/SHA256SUMS".into()]),
            ..HostConfig::default()
        };
        let mut resolved = settings("core", Some(host.clone()));
        resolved.extend(settings("app", Some(host)));

        let planned = planned_host_releases(&plan, &modules, &targets(), &resolved).unwrap();

        assert_eq!(
            planned.len(),
            1,
            "one tag is one hosted Release: {planned:?}"
        );
        assert_eq!(planned[0].release.tag, "v0.1.1");
        // The shared asset is uploaded once, not once per contributing module.
        assert_eq!(planned[0].release.assets.len(), 1);
        // Identical per-module notes collapse instead of repeating.
        assert_eq!(planned[0].release.notes, "- did a thing");
    }

    #[test]
    fn mixed_library_and_binary_modules_collapse_into_one_release() {
        // A mixed repo: a registry library (`corelib`) contributes release
        // notes only, while a binary app (`app`) contributes signed archives.
        // Both render the shared `v{version}` tag, so they collapse into one
        // hosted Release whose notes and assets are the union of the two
        // per-module contributions — assets owned by the binary, notes by both.
        let shared = |name: &str, notes: &str| {
            let mut entry = entry(name, None);
            entry.tag_format = Some("v{version}".into());
            entry.changelog = ChangelogEntry::new(mkey(name), "changed", vec![notes.to_string()]);
            entry
        };
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                shared("corelib", "- library change"),
                shared("app", "- app change"),
            ],
        );
        let modules = vec![module("corelib"), module("app")];

        // The library declares a forge but no assets (notes only); the binary
        // declares the archives and checksum manifest.
        let library_host = HostConfig {
            forge: Some("github".into()),
            ..HostConfig::default()
        };
        let binary_host = HostConfig {
            forge: Some("github".into()),
            assets: Some(vec![
                "dist/app-x86_64-unknown-linux-gnu.tar.gz".into(),
                "dist/SHA256SUMS".into(),
            ]),
            ..HostConfig::default()
        };
        let mut resolved = settings("corelib", Some(library_host));
        resolved.extend(settings("app", Some(binary_host)));

        let planned = planned_host_releases(&plan, &modules, &targets(), &resolved).unwrap();

        assert_eq!(
            planned.len(),
            1,
            "one shared tag is one Release: {planned:?}"
        );
        let release = &planned[0].release;
        assert_eq!(release.tag, "v0.1.1");
        // Assets come solely from the binary module.
        let asset_paths: Vec<_> = release
            .assets
            .iter()
            .map(|asset| asset.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            asset_paths,
            vec![
                "dist/app-x86_64-unknown-linux-gnu.tar.gz".to_string(),
                "dist/SHA256SUMS".to_string()
            ]
        );
        // Notes are the merged union of both modules' distinct bodies.
        assert!(
            release.notes.contains("- library change"),
            "{}",
            release.notes
        );
        assert!(release.notes.contains("- app change"), "{}", release.notes);
    }

    #[test]
    fn merge_rejects_one_asset_path_contributed_with_divergent_labels() {
        let hosted = |label: Option<&str>| {
            let mut asset = ReleaseAsset::new("dist/app.tgz");
            if let Some(label) = label {
                asset = asset.with_label(label);
            }
            HostedRelease::new("v0.1.1", "v0.1.1", "notes").with_assets(vec![asset])
        };
        let planned_of = |release| PlannedHostRelease {
            forge: "github".to_string(),
            member: None,
            release,
        };

        // Same path, same label: deduped to one asset without error.
        let mut planned = vec![planned_of(hosted(Some("App")))];
        merge_planned(&mut planned, planned_of(hosted(Some("App"))), &mkey("app")).unwrap();
        assert_eq!(planned[0].release.assets.len(), 1);

        // Same path, divergent labels: fails closed instead of last-writer-wins.
        let mut planned = vec![planned_of(hosted(Some("First")))];
        let error = merge_planned(
            &mut planned,
            planned_of(hosted(Some("Second"))),
            &mkey("app"),
        )
        .expect_err("a divergent label for one asset path must fail closed");
        assert_eq!(error.code(), ErrorCode::InvalidInput);
    }

    #[test]
    fn planned_releases_error_when_a_planned_module_has_no_resolved_settings() {
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry("core", None)]);
        let modules = vec![module("core")];

        // Settings resolved for a different module leave the planned module unresolved:
        // an internal inconsistency, not a legitimate skip.
        let result = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("other", Some(github_host())),
        );
        let Err(error) = result else {
            panic!("missing settings must surface a typed error");
        };
        assert_eq!(error.code(), ErrorCode::Internal);
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
        )
        .unwrap();

        assert!(planned[0].release.prerelease);
    }

    #[test]
    fn a_prerelease_version_marks_the_hosted_release_as_a_prerelease() {
        // No `--pre` channel: the declared version itself carries the prerelease
        // identifier, as it does for a first release cut from `0.1.0-alpha.1`.
        let mut prerelease_entry = entry("core", None);
        let version = Version::parse("0.1.0-alpha.1").unwrap();
        prerelease_entry.planned_version = Some(version.clone());
        prerelease_entry.mutation = ReleaseMutation::version(version);
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![prerelease_entry]);
        let modules = vec![module("core")];

        let planned = planned_host_releases(
            &plan,
            &modules,
            &targets(),
            &settings("core", Some(github_host())),
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

        let planned =
            planned_host_releases(&plan, &modules, &targets(), &settings("core", Some(host)))
                .unwrap();

        let release = &planned[0].release;
        assert!(release.draft);
        // Explicit config wins over the derived prerelease channel.
        assert!(!release.prerelease);
        assert_eq!(release.notes, "handcrafted");
        assert_eq!(release.assets.len(), 1);
        // Asset paths stay project-relative through the port; resolution to an
        // absolute filesystem path is deferred to fingerprint time.
        assert_eq!(
            release.assets[0].path,
            Path::new("target/toven/release/core.cdx.json")
        );
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
        let planned = planned_host_releases(&plan, &modules, &targets(), &all).unwrap();

        let host = FakeReleaseHost::new().with_outcome(HostReleaseOutcome::AlreadyComplete);
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
        )
        .unwrap();

        let host = FakeReleaseHost::new().with_outcome(HostReleaseOutcome::AlreadyComplete);
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

    #[test]
    fn build_hosts_resolves_gitlab() {
        let host = HostConfig {
            forge: Some("gitlab".into()),
            ..HostConfig::default()
        };
        let hosts = build_hosts(&settings("core", Some(host))).unwrap();
        assert!(hosts.contains_key("gitlab"));
    }
}
