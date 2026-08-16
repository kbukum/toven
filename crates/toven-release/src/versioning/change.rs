//! Release change detection.
//!
//! Detection is member-scoped: each member repo contributes its own worktree
//! status, tag namespace, and configured baseline, while the changed seeds are
//! still resolved against the one federated umbrella graph. A single-repo
//! project is the N=1 degenerate member (no id, empty path prefix).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{EcosystemId, MemberId, Module, ModuleKey};
use toven_ports::{
    BaselineSpec, ChangeRecord, CommitSummary, Oid, ReleaseAdapter, Reporter, TagRef, TagScheme,
};

use toven_core::federation::baseline::{MemberVcsReader, MemberVcsReaders};
use toven_core::plan::PlanContext;

use crate::BaselineSource;
use crate::{ReleaseBaseline, ReleaseTargets, ResolvedReleaseSettings};

/// Per-module change-detection output.
#[derive(Debug, Clone)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct ReleaseChanges {
    pub(crate) changed: BTreeSet<ModuleKey>,
    pub(crate) records: BTreeMap<ModuleKey, Vec<ChangeRecord>>,
    pub(crate) commits: BTreeMap<ModuleKey, Vec<CommitSummary>>,
    pub(crate) baselines: BTreeMap<ModuleKey, ReleaseBaseline>,
}

struct MemberSnapshot {
    worktree: Vec<ChangeRecord>,
    tags: Vec<TagRef>,
}

/// Detect modules in dependency-first decision order and resolve each one before advancing.
///
/// # Errors
/// Propagates VCS, release-target, callback, and reporter failures.
#[allow(
    clippy::redundant_pub_crate,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub(crate) fn detect_in_order(
    context: &PlanContext,
    base_override: Option<&str>,
    readers: &MemberVcsReaders<'_>,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    intent: crate::versioning::bump::CutIntent,
    modules: &[ModuleKey],
    reporter: &mut dyn Reporter,
    mut resolve: impl FnMut(&Module, &ReleaseChanges, &mut dyn Reporter) -> AppResult<()>,
) -> AppResult<ReleaseChanges> {
    let mut changes = ReleaseChanges {
        changed: BTreeSet::new(),
        records: BTreeMap::new(),
        commits: BTreeMap::new(),
        baselines: BTreeMap::new(),
    };
    let mut snapshots = BTreeMap::new();
    for reader in readers.entries() {
        snapshots.insert(
            reader.member().cloned(),
            MemberSnapshot {
                worktree: reader.umbrella_records(&reader.reader().worktree_status()?),
                tags: reader.reader().list_tags(None)?,
            },
        );
    }

    let reference_globs = version_reference_globs(settings);
    let mut umbrella_schemes: BTreeMap<(Option<MemberId>, EcosystemId), Option<TagScheme>> =
        BTreeMap::new();
    for key in modules {
        let module = context.graph.module(key).ok_or_else(|| {
            AppError::invalid_input("release.modules", format!("unknown module '{key}'"))
        })?;
        let resolved = settings.get(key).ok_or_else(|| {
            AppError::invalid_input(
                "release.settings",
                format!("module '{key}' has no resolved release settings"),
            )
        })?;
        let target = target_for(targets, module).ok_or_else(|| {
            AppError::invalid_input(
                "release.target",
                format!("module '{key}' has no release target"),
            )
        })?;
        let reader = reader_for(readers, module.member.as_ref())?;
        let snapshot = snapshots.get(&module.member).ok_or_else(|| {
            AppError::invalid_input(
                "release.member",
                format!("module '{key}' has no VCS snapshot"),
            )
        })?;
        reporter.emit(&crate::stream::examining_event(key))?;

        let umbrella_key = (module.member.clone(), module.id.ecosystem.clone());
        let umbrella_scheme = if let Some(scheme) = umbrella_schemes.get(&umbrella_key) {
            scheme.clone()
        } else {
            let scheme = train_umbrella_scheme(
                context,
                module.member.as_ref(),
                &module.id.ecosystem,
                targets,
                settings,
            )?;
            umbrella_schemes.insert(umbrella_key, scheme.clone());
            scheme
        };
        let scheme = target.tag_scheme(module, resolved.tag_format.as_deref())?;
        let source = resolve_baseline_source(resolved.baseline, scheme, umbrella_scheme.as_ref())?;
        let Some(spec) = baseline_spec(
            module,
            base_override,
            &snapshot.tags,
            &source,
            reader.reader(),
            target,
            &mut changes.baselines,
        )?
        else {
            changes.changed.insert(key.clone());
            changes.records.insert(key.clone(), Vec::new());
            changes.commits.insert(
                key.clone(),
                collect_commits(reader, &changes.baselines, module)?,
            );
            resolve(module, &changes, reporter)?;
            continue;
        };
        let mut module_changes = reader.umbrella_records(&reader.reader().changed_since(&spec)?);
        module_changes.extend(snapshot.worktree.iter().cloned());
        if !reference_globs.is_empty() {
            module_changes
                .retain(|record| !path_matches_any(&reference_globs, record.path.as_path()));
        }
        // Release gates fail-closed: a changed path that attributes to no single
        // module (workspace-root / CI / docs / skills) or only to a workspace
        // blast-radius glob (a shared Cargo.lock) is not release-relevant and must
        // bump nothing. A real first-party dependency floor still reaches
        // dependents through the graph cascade, never through blanket activation.
        let seeds = toven_core::plan::changed_seeds(
            &module_changes,
            &context.graph,
            &context.federation,
            toven_core::plan::AttributionPolicy::FailClosed,
        );
        let force_maintainer =
            intent.forces_maintainer_owned() && resolved.entrypoint.is_maintainer_owned();
        if seeds.contains(key) || force_maintainer {
            changes.changed.insert(key.clone());
            changes.records.insert(
                key.clone(),
                toven_core::plan::changed_records_for_module(
                    module,
                    &module_changes,
                    &context.federation,
                ),
            );
            changes.commits.insert(
                key.clone(),
                collect_commits(reader, &changes.baselines, module)?,
            );
        }
        resolve(module, &changes, reporter)?;
    }
    Ok(changes)
}

fn reader_for<'a>(
    readers: &'a MemberVcsReaders<'a>,
    member: Option<&MemberId>,
) -> AppResult<&'a MemberVcsReader<'a>> {
    readers
        .entries()
        .iter()
        .find(|reader| reader.member() == member)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.member",
                member.map_or_else(
                    || "project has no VCS reader".to_string(),
                    |member| format!("member '{member}' has no VCS reader"),
                ),
            )
        })
}

/// Collect the Conventional-Commit history a changed module's changelog is
/// generated from: the commits from the module's release baseline to `HEAD`,
/// scoped to the module's own directory.
///
/// A module with no baseline (a first release) has no prior tag to diff
/// against, so its whole path history is walked. Scoping by [`Module::root`]
/// keeps each module's changelog to its own changes; workspace-root noise
/// (lockfiles, CI config) attaches to no module.
fn collect_commits(
    reader: &MemberVcsReader<'_>,
    baselines: &BTreeMap<ModuleKey, ReleaseBaseline>,
    module: &Module,
) -> AppResult<Vec<CommitSummary>> {
    let since = baselines
        .get(&module.key())
        .and_then(|baseline| baseline.tag.as_deref());
    reader
        .reader()
        .commits_since(since, Some(module.root.as_path()))
}

/// Resolve the diff baseline for one module's release change detection.
///
/// A release baseline answers "what changed **since the last release**". The
/// module's [`BaselineSource`] — its own release tag, a shared umbrella tag, or
/// the registry's max published version (composed by
/// [`default_baseline_source`]) — is resolved to a concrete
/// [`ReleaseBaseline`] by the shared [`resolve_baseline`] resolver. `--base`
/// overrides the diff ref explicitly *when a released anchor exists*, while the
/// anchor's version continues to anchor idempotency.
///
/// When the source resolves no anchor at all the module has never been
/// released, so `None` is returned and the caller treats the module as an
/// *initial release*: every module is unreleased, and nothing has been
/// published yet to diff against. `--base` is deliberately **not** honored in
/// that case, and neither is a branch ref such as `[project].base_ref` —
/// diffing a never-released module against `origin/main` reports no changes on
/// that branch and would silently plan an empty first release.
fn baseline_spec(
    module: &Module,
    base_override: Option<&str>,
    tags: &[TagRef],
    source: &BaselineSource,
    reader: &dyn toven_ports::VcsReader,
    version_source: &dyn toven_ports::VersionSource,
    baselines: &mut BTreeMap<ModuleKey, ReleaseBaseline>,
) -> AppResult<Option<BaselineSpec>> {
    let baseline = crate::versioning::baseline::resolve_baseline(
        module,
        source,
        reader,
        version_source,
        tags,
    )?;

    // No anchor at all: the module has never been released, so it is always an
    // initial release. `--base` is not honored here — a never-released module
    // has nothing to diff against, and letting a branch ref stand in would
    // silently plan an empty first release.
    if baseline.is_initial() {
        baselines.insert(module.key(), baseline);
        return Ok(None);
    }

    // `--base` overrides the diff ref (default: the release tag commit) while the
    // tag still anchors idempotency. A released baseline can still lack a diff
    // commit — a registry-anchored version with no tag cut yet — in which case
    // there is no ref to diff against: record the baseline (it still anchors
    // idempotency) and report no diff spec rather than passing an empty ref into
    // the VCS.
    let Some(diff_ref) = diff_ref(base_override, baseline.target.as_ref()) else {
        baselines.insert(module.key(), baseline);
        return Ok(None);
    };
    baselines.insert(module.key(), baseline);
    Ok(Some(BaselineSpec::explicit(diff_ref)))
}

/// Resolve the [`BaselineSource`] for a module from its configured `baseline`
/// selector, the module's own tag scheme, and the member's umbrella tag scheme.
///
/// The `baseline` selector is resolved from config, folded over the ecosystem
/// adapter's default in [`resolve_release_settings`](crate::planning::plan), so
/// in the release pipeline it is always `Some`: a registry-backed ecosystem
/// (Rust) resolves `registry+umbrella` when its train declares an umbrella
/// module (the `max(registry, umbrella-tag)` composition, where crates carry
/// per-crate tag schemes the single umbrella tag never matches) and `own-tag`
/// otherwise, while a per-module-tag ecosystem (Go) resolves `own-tag`. `None`
/// is a defensive fallback that anchors on the module's own tag.
///
/// # Errors
/// A source that references the umbrella tag (`umbrella-tag`,
/// `registry+umbrella`) requires the member to declare an umbrella module. Plan
/// validation rejects the mismatch up front; this is the defense-in-depth guard
/// that fails closed with a typed error rather than silently degrading.
fn resolve_baseline_source(
    config: Option<toven_ports::BaselineSourceConfig>,
    own_scheme: TagScheme,
    umbrella_scheme: Option<&TagScheme>,
) -> AppResult<BaselineSource> {
    use toven_ports::BaselineSourceConfig;

    let umbrella = |scheme: Option<&TagScheme>| {
        scheme.cloned().map(BaselineSource::umbrella_tag).ok_or_else(|| {
            rskit_errors::AppError::invalid_input(
                "release.baseline",
                "an umbrella-anchored baseline requires the member to declare an umbrella module, \
                 but none is declared",
            )
        })
    };

    match config {
        // The pipeline folds the adapter default upstream, so an unset selector
        // only reaches here off the release path; fall back to the module's own
        // tag rather than inferring an umbrella anchor twice.
        None | Some(BaselineSourceConfig::OwnTag) => Ok(BaselineSource::own_tag(own_scheme)),
        Some(BaselineSourceConfig::UmbrellaTag) => umbrella(umbrella_scheme),
        Some(BaselineSourceConfig::Registry) => Ok(BaselineSource::registry(
            BaselineSource::own_tag(own_scheme),
        )),
        Some(BaselineSourceConfig::RegistryUmbrella) => {
            Ok(BaselineSource::registry(umbrella(umbrella_scheme)?))
        }
        Some(other) => Err(rskit_errors::AppError::invalid_input(
            "release.baseline",
            format!("unsupported baseline source '{}'", other.as_str()),
        )),
    }
}

/// The umbrella module's tag scheme for one release *train* — a member scoped
/// to a single ecosystem — when that train declares an umbrella module.
///
/// The umbrella tag anchors every train member's baseline in an umbrella
/// layout, so its scheme is resolved once per train rather than per module. A
/// train with no umbrella module returns `None` and each of its modules keeps
/// its own-tag baseline — so declaring a Rust umbrella never perturbs a Go
/// train in the same member, whose modules stay on their own tags.
///
/// # Errors
/// A train that declares more than one `umbrella = true` module is a
/// fail-closed configuration error: the umbrella tag would be ambiguous. This is
/// the defense-in-depth guard on the unset-baseline default path, which infers
/// the umbrella anchor from umbrella presence and so never reaches the explicit
/// selector check in `validate_tag_mode_and_baseline`. Also propagates
/// [`TagGrammar::tag_scheme`](toven_ports::TagGrammar::tag_scheme) failures for
/// the umbrella module's configured tag format.
fn train_umbrella_scheme(
    context: &PlanContext,
    member: Option<&toven_model::MemberId>,
    ecosystem: &EcosystemId,
    targets: &ReleaseTargets,
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<Option<TagScheme>> {
    let mut umbrella: Option<(&Module, TagScheme)> = None;
    for module in context
        .federation
        .modules
        .iter()
        .filter(|module| module.member.as_ref() == member && &module.id.ecosystem == ecosystem)
    {
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        if !resolved.umbrella {
            continue;
        }
        let Some(target) = target_for(targets, module) else {
            continue;
        };
        let scheme = target.tag_scheme(module, resolved.tag_format.as_deref())?;
        if let Some((existing, _)) = &umbrella {
            return Err(rskit_errors::AppError::invalid_input(
                "release.umbrella",
                format!(
                    "ecosystem '{ecosystem}' declares more than one umbrella module ('{}' and \
                     '{}'); a train has a single umbrella representative",
                    existing.key(),
                    module.key()
                ),
            ));
        }
        umbrella = Some((module, scheme));
    }
    Ok(umbrella.map(|(_, scheme)| scheme))
}

/// The diff ref a baseline compares its files against.
///
/// The explicit `--base` wins when the user gave one; otherwise the baseline's
/// own anchor commit is used. `None` means no ref is available — a released
/// baseline with no diff commit (e.g. a registry-anchored version with no tag
/// cut yet) — so the caller records the baseline but plans no file diff rather
/// than passing an empty ref into the VCS.
fn diff_ref(base_override: Option<&str>, target: Option<&Oid>) -> Option<String> {
    match (base_override, target) {
        (Some(base), _) => Some(base.to_string()),
        (None, Some(target)) => Some(target.as_str().to_string()),
        (None, None) => None,
    }
}

fn target_for<'a>(targets: &'a ReleaseTargets, module: &Module) -> Option<&'a dyn ReleaseAdapter> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
}

/// The repo-scoped union of the version-reference file globs declared across a
/// project's resolved release settings.
fn version_reference_globs(settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>) -> Vec<String> {
    let mut globs = BTreeSet::new();
    for resolved in settings.values() {
        for reference in &resolved.version_references {
            for glob in &reference.files {
                globs.insert(glob.clone());
            }
        }
    }
    globs.into_iter().collect()
}

/// Whether a repo-relative path matches any of the version-reference globs.
///
/// The change seam renders repo-relative records with a leading `./` (the
/// single-repo member prefix), so the rendering is normalized before matching a
/// glob authored without that prefix (e.g. `README.md`, `crates/*/README.md`).
fn path_matches_any(globs: &[String], path: &std::path::Path) -> bool {
    let rendered = path.to_string_lossy().replace('\\', "/");
    let normalized = rendered.strip_prefix("./").unwrap_or(&rendered);
    globs
        .iter()
        .any(|glob| rskit_util::glob::glob_match(glob, normalized))
}

#[cfg(test)]
mod tests {
    use toven_ports::{BaselineSourceConfig, Oid, TagScheme};

    use super::{diff_ref, resolve_baseline_source};
    use crate::BaselineSource;

    fn own() -> TagScheme {
        TagScheme::new("rust/core@", "")
    }

    fn umbrella() -> TagScheme {
        TagScheme::new("v", "")
    }

    #[test]
    fn unset_baseline_falls_back_to_own_tag() {
        // The pipeline folds the adapter default upstream, so an unset selector
        // only reaches the resolver off the release path; it anchors on the
        // module's own tag rather than inferring an umbrella anchor.
        let source = resolve_baseline_source(None, own(), None).expect("resolves");
        assert!(matches!(source, BaselineSource::OwnTag { .. }));
    }

    #[test]
    fn unset_baseline_ignores_the_umbrella_scheme() {
        let source = resolve_baseline_source(None, own(), Some(&umbrella())).expect("resolves");
        assert!(matches!(source, BaselineSource::OwnTag { .. }));
    }

    #[test]
    fn explicit_own_tag_ignores_the_umbrella_scheme() {
        let source =
            resolve_baseline_source(Some(BaselineSourceConfig::OwnTag), own(), Some(&umbrella()))
                .expect("resolves");
        assert!(matches!(source, BaselineSource::OwnTag { .. }));
    }

    #[test]
    fn explicit_registry_anchors_the_diff_on_the_own_tag() {
        let source = resolve_baseline_source(Some(BaselineSourceConfig::Registry), own(), None)
            .expect("resolves");
        assert!(matches!(
            source,
            BaselineSource::Registry { diff } if matches!(*diff, BaselineSource::OwnTag { .. })
        ));
    }

    #[test]
    fn registry_umbrella_composes_registry_over_the_umbrella_tag() {
        let source = resolve_baseline_source(
            Some(BaselineSourceConfig::RegistryUmbrella),
            own(),
            Some(&umbrella()),
        )
        .expect("resolves");
        assert!(matches!(
            source,
            BaselineSource::Registry { diff } if matches!(*diff, BaselineSource::UmbrellaTag { .. })
        ));
    }

    #[test]
    fn umbrella_backed_source_without_an_umbrella_scheme_fails_closed() {
        let error = resolve_baseline_source(Some(BaselineSourceConfig::UmbrellaTag), own(), None)
            .expect_err("umbrella-anchored baseline requires an umbrella module");
        assert!(error.to_string().contains("umbrella"), "{error}");
    }

    #[test]
    fn explicit_base_override_wins_over_the_anchor_commit() {
        let target = Oid::new("anchor");
        assert_eq!(
            diff_ref(Some("origin/main"), Some(&target)).as_deref(),
            Some("origin/main")
        );
    }

    #[test]
    fn absent_override_falls_back_to_the_anchor_commit() {
        let target = Oid::new("anchor");
        assert_eq!(diff_ref(None, Some(&target)).as_deref(), Some("anchor"));
    }

    #[test]
    fn a_released_baseline_with_no_diff_commit_yields_no_ref() {
        // A registry-anchored baseline can carry a version but no tag commit;
        // without an explicit `--base` there is nothing to diff against, so no
        // empty ref is manufactured for the VCS.
        assert_eq!(diff_ref(None, None), None);
    }
}
