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

use std::collections::BTreeSet;

use rskit_errors::AppResult;
use toven_ports::{Provider, Reporter};

use super::host;
use super::plan::{plan_with_context, release_targets, resolve_release_settings};
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
/// # Errors
/// Propagates configuration/discovery/graph failures, release-plan failures,
/// release-apply failures (guardrails, mutation, tagging, publishing), and
/// hosted-release failures.
#[allow(clippy::too_many_arguments)]
pub fn release_run(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    repos: &MemberReleaseRepos<'_>,
    overrides: &BumpOverrides,
    reporter: &mut dyn Reporter,
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
    let plan = plan_with_context(&context, request, readers, overrides, &targets)?;
    let mut stats =
        release_apply_by_member(&plan, &context.federation.modules, &targets, repos, options)?;

    // The hosted-release phase runs after a pushing publish: it needs the pushed
    // tag on the forge to cut a Release against.
    if options.publish && !options.no_push {
        let settings = resolve_release_settings(&context, &targets)?;
        let pushed_members = plan
            .entries
            .iter()
            .filter(|entry| {
                settings
                    .get(&entry.module)
                    .is_some_and(|resolved| resolved.push)
            })
            .map(|entry| entry.module.member.clone())
            .collect::<BTreeSet<_>>();
        let planned = host::planned_host_releases(
            &plan,
            &context.federation.modules,
            &targets,
            &settings,
            request.project_root.as_path(),
        )?;
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
            )?;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, CommonEcosystemConfig, DiscoverResponse,
        HostConfig, Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter,
        RecordingReporter,
    };

    use super::release_run;
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
        let mut response = DiscoverResponse::new(eid());
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
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
            .with_release_target(FakeReleaseTarget::new());
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
}
