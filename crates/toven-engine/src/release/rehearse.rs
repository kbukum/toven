//! Read-only rehearsal of the release publish loop.
//!
//! Resolves the same release plan a real run would, then classifies every
//! planned release against the registry's reported versions — reporting the
//! deterministic publish order and a per-module would-publish/already-published
//! verdict without applying any mutation, cutting any tag, or invoking any
//! publish.

use std::collections::BTreeSet;

use rskit_errors::AppResult;
use toven_ports::{Provider, Reporter};

use super::host;
use super::plan::{plan_with_context, release_targets, resolve_release_settings};
use super::{
    BumpOverrides, HostRehearsal, PublishDecision, RehearsalVerdict, ReleasePlan, ReleaseRehearsal,
};
use crate::config::Document;
use crate::federation::baseline::MemberVcsReaders;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// Rehearse the release publish loop without mutating anything.
///
/// Reuses the release PLAN cut, whose per-entry `publish_needed` already folds
/// in the publish loop's idempotency query
/// ([`ReleaseTarget::published_versions`](toven_ports::ReleaseTarget::published_versions)),
/// to report what a real publish would do. It also resolves — but never cuts —
/// the hosted forge Releases a real run would create, so `--dry-run` reports
/// the hosted release plan (tag, flags, assets) while invoking no forge CLI. It
/// never calls `apply_release`, `package`, `publish`, or any host, so no
/// manifest, tag, commit, registry entry, or forge Release is touched.
/// `overrides` carry the per-run bump argv.
///
/// # Errors
/// Propagates configuration/discovery/graph failures and release-plan failures.
pub fn release_rehearse(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    overrides: &BumpOverrides,
    reporter: &mut dyn Reporter,
    no_push: bool,
) -> AppResult<ReleaseRehearsal> {
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
    let settings = resolve_release_settings(&context, &targets)?;
    let pushed_members = if no_push {
        BTreeSet::new()
    } else {
        plan.entries
            .iter()
            .filter(|entry| {
                settings
                    .get(&entry.module)
                    .is_some_and(|resolved| resolved.push)
            })
            .map(|entry| entry.module.member.clone())
            .collect()
    };
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
    Ok(rehearse_plan(
        &plan,
        &planned,
        request.project_root.as_path(),
    ))
}

/// Classify every planned release (an entry with a planned version) against the
/// plan's publish verdict, preserving the plan's deterministic publish order,
/// and carry the hosted forge Releases a real run would cut.
///
/// A `publish_needed` entry is classified `would-publish`; one the planner
/// already found on the registry is classified `already-published`.
fn rehearse_plan(
    plan: &ReleasePlan,
    planned: &[host::PlannedHostRelease],
    project_root: &std::path::Path,
) -> ReleaseRehearsal {
    let verdicts = plan
        .entries
        .iter()
        .filter_map(|entry| {
            entry
                .planned_version
                .as_ref()
                .map(|version| RehearsalVerdict {
                    module: entry.module.clone(),
                    version: version.clone(),
                    decision: if entry.publish_needed {
                        PublishDecision::WouldPublish
                    } else {
                        PublishDecision::AlreadyPublished
                    },
                })
        })
        .collect();
    let hosted = planned
        .iter()
        .map(|entry| HostRehearsal {
            forge: entry.forge.clone(),
            tag: entry.release.tag.clone(),
            draft: entry.release.draft,
            prerelease: entry.release.prerelease,
            assets: entry
                .release
                .assets
                .iter()
                .map(|asset| {
                    asset
                        .path
                        .strip_prefix(project_root)
                        .unwrap_or(&asset.path)
                        .display()
                        .to_string()
                })
                .collect(),
        })
        .collect();
    ReleaseRehearsal::new(plan.policy, verdicts, hosted)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, CommonEcosystemConfig, DiscoverResponse,
        HostConfig, Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
        ReleaseCall,
    };

    use super::release_rehearse;
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::federation::baseline::MemberVcsReaders;
    use crate::plan::{PlanRequest, Selection};
    use crate::release::BumpOverrides;
    use crate::release::PublishDecision;

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

    fn setup(target: FakeReleaseTarget) -> (FakeProvider, FakeVcsReader) {
        setup_with_common(target, CommonEcosystemConfig::default())
    }

    fn setup_with_common(
        target: FakeReleaseTarget,
        common: CommonEcosystemConfig,
    ) -> (FakeProvider, FakeVcsReader) {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_common(common)
            .with_release_target(target);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let vcs = FakeVcsReader::new().with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
        (provider, vcs)
    }

    #[test]
    fn rehearsal_reports_would_publish_and_mutates_nothing() {
        let target = FakeReleaseTarget::new();
        let (provider, vcs) = setup(target.clone());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let rehearsal = release_rehearse(
            &request(),
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
            false,
        )
        .unwrap();

        assert_eq!(rehearsal.verdicts.len(), 1);
        let verdict = &rehearsal.verdicts[0];
        assert_eq!(verdict.version, Version::new(0, 1, 1));
        assert_eq!(verdict.decision, PublishDecision::WouldPublish);

        // No mutation: only the read-only idempotency query was made.
        let calls = target.calls();
        assert!(
            calls.iter().all(|call| !matches!(
                call,
                ReleaseCall::ApplyRelease { .. }
                    | ReleaseCall::Publish(_)
                    | ReleaseCall::Package(_)
            )),
            "rehearsal must not mutate: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|call| matches!(call, ReleaseCall::PublishedVersions(_)))
        );
    }

    #[test]
    fn rehearsal_marks_already_published_versions_as_skips() {
        let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);
        let (provider, vcs) = setup(target);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let rehearsal = release_rehearse(
            &request(),
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
            false,
        )
        .unwrap();

        assert_eq!(
            rehearsal.verdicts[0].decision,
            PublishDecision::AlreadyPublished
        );
    }

    #[test]
    fn rehearsal_carries_the_hosted_release_plan_without_invoking_a_forge() {
        let target = FakeReleaseTarget::new();
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".into()),
                    draft: Some(true),
                    assets: Some(vec!["dist/core.tgz".into()]),
                    ..HostConfig::default()
                }),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let (provider, vcs) = setup_with_common(target.clone(), common);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let rehearsal = release_rehearse(
            &request(),
            &document(),
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
            false,
        )
        .unwrap();

        assert_eq!(rehearsal.hosted.len(), 1);
        let hosted = &rehearsal.hosted[0];
        assert_eq!(hosted.forge, "github");
        assert_eq!(hosted.tag, "rust/core@0.1.1");
        assert!(hosted.draft);
        // Rehearsal asset paths stay project-relative, never absolutized.
        assert_eq!(hosted.assets, vec!["dist/core.tgz".to_string()]);

        // Rehearsal resolves the hosted plan but never mutates or publishes.
        let calls = target.calls();
        assert!(
            calls.iter().all(|call| !matches!(
                call,
                ReleaseCall::ApplyRelease { .. }
                    | ReleaseCall::Publish(_)
                    | ReleaseCall::Package(_)
            )),
            "rehearsal must not mutate: {calls:?}"
        );
    }

    #[test]
    fn rehearsal_omits_hosted_release_when_push_is_suppressed() {
        for (push, no_push) in [(false, false), (true, true)] {
            let common = CommonEcosystemConfig {
                release: ReleaseConfig {
                    push: Some(push),
                    host: Some(HostConfig {
                        forge: Some("github".into()),
                        ..HostConfig::default()
                    }),
                    ..ReleaseConfig::default()
                },
                ..CommonEcosystemConfig::default()
            };
            let (provider, vcs) = setup_with_common(FakeReleaseTarget::new(), common);
            let providers: Vec<&dyn Provider> = vec![&provider];
            let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
            let mut reporter = RecordingReporter::new();

            let rehearsal = release_rehearse(
                &request(),
                &document(),
                &providers,
                &readers,
                &BumpOverrides::new(),
                &mut reporter,
                no_push,
            )
            .unwrap();

            assert_eq!(rehearsal.verdicts.len(), 1);
            assert!(
                rehearsal.hosted.is_empty(),
                "push={push}, no_push={no_push}"
            );
        }
    }
}
