//! Combined release facade: PLAN then APPLY in one call for the CLI `release`
//! verb.
//!
//! [`release_plan`](super::release_plan) and the per-member APPLY are exposed
//! separately so each phase is testable in isolation, but a one-shot `toven
//! release` needs the discovered modules and resolved release targets that the
//! PLAN cut computes internally. This facade prepares the front matter once,
//! reuses it for both the plan and the apply, and returns the terminal
//! [`ReleaseStats`] — keeping the discovery/target wiring engine-owned so the
//! CLI stays a thin caller.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::AppResult;
use toven_model::ModuleKey;
use toven_ports::{HookPhase, HookRunner, Provider, Reporter};

use super::host;
use super::plan::{plan_with_context, release_targets, resolve_release_settings};
use super::reconcile;
use super::settings::ResolvedReleaseSettings;
use super::{BumpOverrides, ReleaseApplyOptions, ReleaseStats};
use crate::config::Document;
use crate::federation::baseline::MemberVcsReaders;
use crate::federation::release::{MemberReleaseRepos, release_apply_by_member};
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// Plan and apply a release in one call.
///
/// Prepares the shared PLAN front matter once, derives the release plan and
/// targets from it, then runs the per-member release APPLY tail. `readers` are
/// the per-member change seams and `repos` the per-member commit/tag/push
/// ports; a single-repo project is the N=1 degenerate member. `overrides` carry
/// the per-run bump argv (level flags, set-version, prerelease channel, base,
/// offline).
///
/// When the run publishes and pushes, a config-gated hosted-release phase runs
/// after APPLY: every tagged module whose `[…release].host` names a forge cuts
/// a forge Release over the one topological order. `--no-push` (a non-pushing
/// APPLY) skips the phase, consistent with the tag push it depends on.
///
/// Configured `[…release].hooks` run through the injected [`HookRunner`]: every
/// resolved `pre` reference runs before **any** mutation (a failing `pre` hook
/// aborts the release before the reconcile pre-pass, mutation, or publish), and
/// every resolved `post` reference runs after a fully successful release.
/// References are de-duplicated across modules and run in a deterministic order.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, release-plan failures,
/// pre/post hook failures, release-apply failures (guardrails, mutation,
/// tagging, publishing), and hosted-release failures.
#[allow(clippy::too_many_arguments)]
pub fn release_run(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    repos: &MemberReleaseRepos<'_>,
    overrides: &BumpOverrides,
    reporter: &mut dyn Reporter,
    hooks: &dyn HookRunner,
    options: &ReleaseApplyOptions,
) -> AppResult<ReleaseStats> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;

    // Resolve settings once up front and reuse the same map for every phase of
    // the run — the lifecycle hooks, the reconcile pre-pass, and the hosted
    // phase — so hook collection and reconciliation see one consistent
    // resolution. `pre` hooks run before the reconcile pre-pass and every
    // mutation below, so a failing gate (e.g. a test task) aborts the release
    // before anything is written, tagged, or published; `post` hooks reuse it
    // after success.
    let settings = resolve_release_settings(&context, &targets)?;
    for reference in collect_hook_refs(&settings, HookPhase::Pre) {
        hooks.run_hook(HookPhase::Pre, &reference)?;
    }

    // Reconcile pre-pass: complete a hosted Release for the already-published
    // current version before planning any bump. A run that published the tag and
    // registry version but failed before cutting the forge Release leaves an
    // immutable, un-hostable state the bump planner can never reach (a changed
    // module always plans a forward bump; an unchanged module is dropped from the
    // plan). Keyed on the published+tagged+unhosted state, this pre-pass is
    // reachable by the automatic `release publish` re-dispatch. It runs only for
    // a pushing publish (the hosted phase depends on the pushed tag) and
    // short-circuits the run only when it actually creates a missing Release, so
    // a legitimate new release is never blocked.
    if options.publish && !options.no_push {
        let hosts = host::build_hosts(&settings)?;
        let mut stats = ReleaseStats::new(0);
        let created = reconcile::reconcile_hosted_releases(
            &context.federation.modules,
            &targets,
            &settings,
            repos,
            &hosts,
            request.project_root.as_path(),
            &mut stats,
        )?;
        if created {
            stats.resumed = true;
            reporter.emit(&toven_model::Event::Warning {
                message: "the tag and registry version for the current release are already \
                          published; completing only the missing hosted Release and skipping the \
                          manifest mutation, commit, tag, push, and registry publish"
                    .to_string(),
            })?;
            return Ok(stats);
        }
    }

    let plan = plan_with_context(
        &context,
        request,
        readers,
        overrides,
        &targets,
        super::bump::CutIntent::Mutate,
    )?;
    let mut stats =
        release_apply_by_member(&plan, &context.federation.modules, &targets, repos, options)?;

    // A resumed apply skipped the already-applied git mutation phase; surface it
    // so the operator sees why no commit/tag/push happened and that the run is
    // completing only the missing publish and hosted-release work.
    if stats.resumed {
        reporter.emit(&toven_model::Event::Warning {
            message: "release commit and tags already exist for the planned version; skipping \
                      the manifest mutation, commit, tag, and push, and completing only the \
                      idempotent publish and hosted-release phases"
                .to_string(),
        })?;
    }

    // The hosted-release phase runs after a pushing publish: it needs the pushed
    // tag on the forge to cut a Release against.
    if options.publish && !options.no_push {
        let pushed_members = plan
            .entries
            .iter()
            .filter(|entry| {
                settings
                    .get(&entry.module)
                    .is_some_and(|resolved| resolved.push.permits_push())
            })
            .map(|entry| entry.module.member.clone())
            .collect::<BTreeSet<_>>();
        let planned =
            host::planned_host_releases(&plan, &context.federation.modules, &targets, &settings)?;
        let planned = planned
            .into_iter()
            .filter(|entry| pushed_members.contains(&entry.member))
            .collect::<Vec<_>>();
        if !planned.is_empty() {
            let hosts = host::build_hosts(&settings)?;
            host::run_host_phase(
                &planned,
                &hosts,
                repos,
                request.project_root.as_path(),
                &mut stats,
            )
            .map_err(|error| {
                super::apply::forward_recovery_error(
                    "the release commit, tags, and registry publication completed",
                    "hosted release",
                    error,
                )
            })?;
        }
    }
    // `post` hooks run only after a fully successful release (the reconcile
    // pre-pass short-circuit above intentionally skips them: it completes a prior
    // release's missing hosted Release, not a fresh mutation).
    for reference in collect_hook_refs(&settings, HookPhase::Post) {
        hooks.run_hook(HookPhase::Post, &reference)?;
    }
    Ok(stats)
}

/// Collect the `phase` hook references across all resolved modules, de-duplicated
/// and in a deterministic order (module-key order, then declaration order within
/// a module). A hook naming the same task in two modules runs once.
fn collect_hook_refs(
    settings: &BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    phase: HookPhase,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut references = Vec::new();
    for resolved in settings.values() {
        let hooks = match phase {
            HookPhase::Pre => &resolved.hooks.pre,
            HookPhase::Post => &resolved.hooks.post,
        };
        for reference in hooks {
            if seen.insert(reference.clone()) {
                references.push(reference.clone());
            }
        }
    }
    references
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, CommonEcosystemConfig, DiscoverResponse,
        HostConfig, Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter,
        RecordingHookRunner, RecordingReporter,
    };

    use super::{ResolvedReleaseSettings, collect_hook_refs, release_run};
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::federation::baseline::MemberVcsReaders;
    use crate::federation::release::{MemberReleaseRepo, MemberReleaseRepos};
    use crate::plan::{PlanRequest, Selection};
    use crate::release::{BumpOverrides, ReleaseApplyOptions};

    fn eid() -> EcosystemId {
        EcosystemId::new("rust").unwrap()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(eid(), name).unwrap()
    }

    fn module(name: &str) -> Module {
        Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap())
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

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid(), RawValue::from(json!({ "release": {} })));
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
            modules: BTreeMap::new(),
            members: Vec::new(),
        }
    }

    fn provider_with_host_and_push(push: bool) -> FakeProvider {
        provider_with_host_push_and_published(push, Vec::new())
    }

    fn provider_with_host_push_and_published(
        push: bool,
        published: Vec<rskit_version::semver::Version>,
    ) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid());
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                registry: Some("crates-io".into()),
                host: Some(HostConfig {
                    forge: Some("github".into()),
                    ..HostConfig::default()
                }),
                push: Some(push),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid())
            .with_response(response)
            .with_common(common)
            .with_release_target(FakeReleaseTarget::new().with_published_versions(published));
        FakeProvider::new(eid()).with_adapter(adapter)
    }

    // A configured hosted release must NOT be cut when the run does not push: the
    // host phase depends on the pushed tag, so `--no-push` skips it.
    #[test]
    fn host_phase_is_skipped_when_the_run_does_not_push() {
        let provider = provider_with_host_and_push(true);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let plan_reader = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&plan_reader, BaselineSpec::explicit("main"));
        let apply_reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c1");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            AbsPath::new("/repo").unwrap().as_path().to_path_buf(),
            &apply_reader,
            &writer,
        )]);
        let mut reporter = RecordingReporter::new();

        let stats = release_run(
            &request(),
            &document(),
            &providers,
            &readers,
            &repos,
            &BumpOverrides::new(),
            &mut reporter,
            &RecordingHookRunner::new(),
            &ReleaseApplyOptions {
                no_push: true,
                publish: true,
                ..ReleaseApplyOptions::default()
            },
        )
        .unwrap();

        assert_eq!(stats.published_modules, 1);
        assert_eq!(stats.hosted_releases, 0);
    }

    #[test]
    fn host_phase_is_skipped_when_member_config_disables_push() {
        let provider = provider_with_host_and_push(false);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let plan_reader = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&plan_reader, BaselineSpec::explicit("main"));
        let apply_reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c1");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            AbsPath::new("/repo").unwrap().as_path().to_path_buf(),
            &apply_reader,
            &writer,
        )]);
        let mut reporter = RecordingReporter::new();

        let stats = release_run(
            &request(),
            &document(),
            &providers,
            &readers,
            &repos,
            &BumpOverrides::new(),
            &mut reporter,
            &RecordingHookRunner::new(),
            &ReleaseApplyOptions {
                no_push: false,
                publish: true,
                ..ReleaseApplyOptions::default()
            },
        )
        .unwrap();

        assert_eq!(stats.hosted_releases, 0);
        assert!(
            !writer
                .writes()
                .iter()
                .any(|write| matches!(write, toven_testkit::VcsWrite::Push { .. }))
        );
    }

    #[test]
    fn a_resumed_apply_skips_git_mutation_and_reports_the_resume() {
        // The planned tag already exists and the version is already published:
        // APPLY resumes — no commit/tag/push — and the operator sees a resume
        // notice. `--no-push` keeps the real hosted-release phase out of this
        // unit test (its `gh` invocation is exercised at the phase level).
        let provider = provider_with_host_push_and_published(
            true,
            vec![rskit_version::semver::Version::new(0, 1, 0)],
        );
        let providers: Vec<&dyn Provider> = vec![&provider];
        let plan_reader = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = MemberVcsReaders::single(&plan_reader, BaselineSpec::explicit("main"));
        let apply_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
            "rust/core@0.1.0",
            toven_ports::Oid::new("deadbee"),
        )]);
        let writer = FakeVcsWriter::new().with_commit_oid("c1");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            AbsPath::new("/repo").unwrap().as_path().to_path_buf(),
            &apply_reader,
            &writer,
        )]);
        let mut reporter = RecordingReporter::new();

        let stats = release_run(
            &request(),
            &document(),
            &providers,
            &readers,
            &repos,
            &BumpOverrides::new(),
            &mut reporter,
            &RecordingHookRunner::new(),
            &ReleaseApplyOptions {
                no_push: true,
                publish: true,
                ..ReleaseApplyOptions::default()
            },
        )
        .unwrap();

        assert!(stats.resumed, "the run is marked resumed");
        assert_eq!(stats.tagged_modules, 0);
        assert_eq!(
            stats.published_modules, 0,
            "the version is already published"
        );
        assert!(
            writer.writes().is_empty(),
            "no commit/tag/push may happen on resume: {:?}",
            writer.writes()
        );
        assert!(
            reporter.events().iter().any(|event| matches!(
                event,
                toven_model::Event::Warning { message } if message.contains("already exist")
            )),
            "the operator sees a resume notice: {:?}",
            reporter.events()
        );
    }

    /// A rust `core` module whose ecosystem release config carries `pre`/`post`
    /// hooks (tag-only, so the unit test needs no forge/registry mutation).
    fn provider_with_hooks(pre: &[&str], post: &[&str]) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid());
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                push: Some(false),
                hooks: Some(toven_ports::HooksConfig {
                    pre: pre
                        .iter()
                        .map(|reference| (*reference).to_string())
                        .collect(),
                    post: post
                        .iter()
                        .map(|reference| (*reference).to_string())
                        .collect(),
                }),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid())
            .with_response(response)
            .with_common(common)
            .with_release_target(FakeReleaseTarget::new());
        FakeProvider::new(eid()).with_adapter(adapter)
    }

    fn changed_core_readers(plan_reader: &FakeVcsReader) -> MemberVcsReaders<'_> {
        MemberVcsReaders::single(plan_reader, BaselineSpec::explicit("main"))
    }

    // A failing `pre` hook must abort the release before ANY mutation: no
    // commit, tag, or push may happen, so a maintainer's gate is honored.
    #[test]
    fn a_failing_pre_hook_aborts_the_release_before_any_mutation() {
        let provider = provider_with_hooks(&["gate"], &["notify"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let plan_reader = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = changed_core_readers(&plan_reader);
        let apply_reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c1");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            AbsPath::new("/repo").unwrap().as_path().to_path_buf(),
            &apply_reader,
            &writer,
        )]);
        let mut reporter = RecordingReporter::new();
        let hooks = RecordingHookRunner::failing_on("gate");

        let error = release_run(
            &request(),
            &document(),
            &providers,
            &readers,
            &repos,
            &BumpOverrides::new(),
            &mut reporter,
            &hooks,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a failing pre hook fails the release closed");

        assert!(error.to_string().contains("gate"), "{error}");
        assert_eq!(
            hooks.references(toven_ports::HookPhase::Pre),
            vec!["gate".to_string()],
            "the pre hook was attempted"
        );
        assert!(
            hooks.references(toven_ports::HookPhase::Post).is_empty(),
            "no post hook runs once pre aborts"
        );
        assert!(
            writer.writes().is_empty(),
            "no mutation may happen when a pre hook fails: {:?}",
            writer.writes()
        );
    }

    // On a successful release, `pre` hooks run before the mutation and `post`
    // hooks run after it — proving the configured task references execute in the
    // right order around the release.
    #[test]
    fn pre_hooks_run_before_the_mutation_and_post_hooks_after_success() {
        let provider = provider_with_hooks(&["build"], &["notify"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let plan_reader = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        let readers = changed_core_readers(&plan_reader);
        let apply_reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c1");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            AbsPath::new("/repo").unwrap().as_path().to_path_buf(),
            &apply_reader,
            &writer,
        )]);
        let mut reporter = RecordingReporter::new();
        let hooks = RecordingHookRunner::new();

        let stats = release_run(
            &request(),
            &document(),
            &providers,
            &readers,
            &repos,
            &BumpOverrides::new(),
            &mut reporter,
            &hooks,
            &ReleaseApplyOptions::default(),
        )
        .expect("the release runs");

        assert_eq!(stats.tagged_modules, 1, "the module is tagged (mutated)");
        let calls = hooks.calls();
        assert_eq!(
            calls,
            vec![
                toven_testkit::HookCall {
                    phase: toven_ports::HookPhase::Pre,
                    reference: "build".to_string(),
                },
                toven_testkit::HookCall {
                    phase: toven_ports::HookPhase::Post,
                    reference: "notify".to_string(),
                },
            ],
            "pre runs, then the mutation, then post"
        );
    }

    fn settings_with_pre(pre: &[&str]) -> ResolvedReleaseSettings {
        let config = ReleaseConfig {
            hooks: Some(toven_ports::HooksConfig {
                pre: pre
                    .iter()
                    .map(|reference| (*reference).to_string())
                    .collect(),
                post: Vec::new(),
            }),
            ..ReleaseConfig::default()
        };
        ResolvedReleaseSettings::resolve(&config, None).unwrap()
    }

    // `collect_hook_refs` collects across every module in module-key order (not
    // insertion order), preserves each module's declaration order, and runs a
    // reference shared by two modules once.
    #[test]
    fn collect_hook_refs_dedupes_across_modules_in_module_key_order() {
        let mut settings = BTreeMap::new();
        // Insert the later module first so a passing result can only come from
        // the BTreeMap's key ordering, never insertion order.
        settings.insert(
            ModuleKey::bare(mref("zeta")),
            settings_with_pre(&["shared", "z"]),
        );
        settings.insert(
            ModuleKey::bare(mref("alpha")),
            settings_with_pre(&["a", "shared"]),
        );

        let references = collect_hook_refs(&settings, toven_ports::HookPhase::Pre);

        assert_eq!(
            references,
            vec!["a".to_string(), "shared".to_string(), "z".to_string()],
            "alpha (module-key order) first with its declaration order kept, then zeta's \
             new refs, with the shared task deduplicated"
        );
    }
}
