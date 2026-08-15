use std::collections::BTreeMap;

use rskit_config::RawValue;
use rskit_version::semver::Version;
use serde_json::json;
use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleKey, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, BumpLevel, ChangeRecord, ChangeStatus, ChangelogConfig, CommonEcosystemConfig,
    DependentVersion, DiscoverResponse, Oid, PrereleaseConfig, Provider, PublicationPolicy,
    ReleaseConfig, TagRef, TaskIntent,
};
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
};

use super::changelog_required::validate_required_changelogs;
use super::spine::{plan_with_context, release_plan};
use super::targets::release_targets;
use super::validation::{
    check_umbrella_count, reconcile_policy, umbrella_selector, validate_phase_backing_supported,
};
use crate::versioning::bump;
use crate::{
    BumpOverrides, BumpPolicy, BumpReason, BumpSource, ReleasePlan, ResolvedReleaseSettings,
};
use toven_core::config::{Document, ProjectConfig, TovenConfig};
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, Selection, prepare_front};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).unwrap()
}

fn mref(name: &str) -> ModuleRef {
    ModuleRef::new(eid("rust"), name).unwrap()
}

fn module(name: &str, root: &str) -> Module {
    Module::new(mref(name), RepoPath::new(root).unwrap())
}

#[test]
fn a_native_phase_backing_passes_the_support_guard() {
    let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
    assert!(
        validate_phase_backing_supported(&module("core", "core"), &resolved).is_ok(),
        "an unconfigured (native) phase backing must be accepted"
    );
}

#[test]
fn per_module_tag_mode_and_own_tag_baseline_need_no_umbrella() {
    use toven_ports::{BaselineSourceConfig, TagMode};
    assert!(
        umbrella_selector(Some(TagMode::PerModule), Some(BaselineSourceConfig::OwnTag)).is_none()
    );
    assert!(umbrella_selector(None, None).is_none());
    assert!(umbrella_selector(None, Some(BaselineSourceConfig::Registry)).is_none());
}

#[test]
fn umbrella_tag_mode_and_umbrella_baseline_require_an_umbrella() {
    use toven_ports::{BaselineSourceConfig, TagMode};
    assert_eq!(
        umbrella_selector(Some(TagMode::Umbrella), None),
        Some("umbrella")
    );
    assert_eq!(umbrella_selector(Some(TagMode::Both), None), Some("both"));
    assert_eq!(
        umbrella_selector(None, Some(BaselineSourceConfig::UmbrellaTag)),
        Some("umbrella-tag")
    );
    assert_eq!(
        umbrella_selector(None, Some(BaselineSourceConfig::RegistryUmbrella)),
        Some("registry+umbrella")
    );
}

#[test]
fn umbrella_selector_reports_the_tag_mode_before_the_baseline() {
    use toven_ports::{BaselineSourceConfig, TagMode};
    assert_eq!(
        umbrella_selector(
            Some(TagMode::Umbrella),
            Some(BaselineSourceConfig::UmbrellaTag)
        ),
        Some("umbrella")
    );
}

#[test]
fn an_umbrella_selector_with_zero_or_many_umbrellas_fails_closed() {
    let zero = check_umbrella_count("rust:core", "umbrella", 0)
        .expect_err("zero umbrella modules must fail closed");
    assert!(zero.to_string().contains("no umbrella module"), "{zero}");

    let many = check_umbrella_count("rust:core", "umbrella", 2)
        .expect_err("multiple umbrella modules must fail closed");
    assert!(many.to_string().contains("single umbrella"), "{many}");

    assert!(check_umbrella_count("rust:core", "umbrella", 1).is_ok());
}

#[test]
fn a_delegated_delegable_phase_is_accepted() {
    use toven_model::ReleasePhase;
    use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

    let mut phases = BTreeMap::new();
    phases.insert(
        ReleasePhase::Package,
        PhaseConfig {
            backing: PhaseBackingKind::Delegated,
            delegated: Some(DelegatedTool {
                tool: "goreleaser".into(),
                args: Some(vec!["release".into()]),
                preview: vec!["release".into(), "--snapshot".into()],
            }),
        },
    );
    let ecosystem = ReleaseConfig {
        phases: Some(PhasesConfig(phases)),
        ..ReleaseConfig::default()
    };
    let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

    assert!(
        validate_phase_backing_supported(&module("core", "core"), &resolved).is_ok(),
        "delegating the package phase (delegable) must be accepted"
    );
}

#[test]
fn delegating_a_flow_ownership_phase_is_rejected() {
    use toven_model::ReleasePhase;
    use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

    // Publish is a flow-ownership phase: Toven owns registry publication and
    // never delegates it, so a delegated backing must fail closed at plan
    // time rather than degrade to native.
    let mut phases = BTreeMap::new();
    phases.insert(
        ReleasePhase::Publish,
        PhaseConfig {
            backing: PhaseBackingKind::Delegated,
            delegated: Some(DelegatedTool {
                tool: "goreleaser".into(),
                args: Some(vec!["release".into()]),
                preview: vec!["release".into(), "--snapshot".into()],
            }),
        },
    );
    let ecosystem = ReleaseConfig {
        phases: Some(PhasesConfig(phases)),
        ..ReleaseConfig::default()
    };
    let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

    let error = validate_phase_backing_supported(&module("core", "core"), &resolved)
        .expect_err("delegating a flow-ownership phase must fail closed");
    let message = error.to_string();
    assert!(message.contains("publish"), "{message}");
    assert!(message.contains("goreleaser"), "{message}");
    assert!(
        message.contains("never delegates"),
        "the error must explain Toven owns the phase: {message}"
    );
}

#[test]
fn delegating_a_not_yet_wired_delegable_phase_is_rejected() {
    use toven_model::ReleasePhase;
    use toven_ports::{DelegatedTool, PhaseBackingKind, PhaseConfig, PhasesConfig};

    // Provenance is delegable in principle, but its delegated execution is
    // not yet wired at the call site — accepting it would silently run
    // native, so it must fail closed until dispatch lands.
    let mut phases = BTreeMap::new();
    phases.insert(
        ReleasePhase::Provenance,
        PhaseConfig {
            backing: PhaseBackingKind::Delegated,
            delegated: Some(DelegatedTool {
                tool: "goreleaser".into(),
                args: Some(vec!["release".into()]),
                preview: vec!["release".into(), "--snapshot".into()],
            }),
        },
    );
    let ecosystem = ReleaseConfig {
        phases: Some(PhasesConfig(phases)),
        ..ReleaseConfig::default()
    };
    let resolved = ResolvedReleaseSettings::resolve(&ecosystem, None).unwrap();

    let error = validate_phase_backing_supported(&module("core", "core"), &resolved)
        .expect_err("delegating a not-yet-wired phase must fail closed");
    let message = error.to_string();
    assert!(message.contains("provenance"), "{message}");
    assert!(
        message.contains("not yet wired"),
        "the error must explain dispatch is unimplemented: {message}"
    );
}

/// Release tags placing every named module at `0.1.0`, so change detection
/// diffs against a real prior release instead of planning a first release.
fn released_at_0_1_0(modules: &[&str]) -> Vec<TagRef> {
    modules
        .iter()
        .map(|name| TagRef::new(format!("rust/{name}@0.1.0"), Oid::new("cafe")))
        .collect()
}

fn module_for_ecosystem(ecosystem: &str, name: &str, root: &str) -> Module {
    Module::new(
        ModuleRef::new(eid(ecosystem), name).unwrap(),
        RepoPath::new(root).unwrap(),
    )
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

fn document_for_ecosystem(ecosystem: &str, release: &serde_json::Value) -> Document {
    let mut ecosystems = BTreeMap::new();
    ecosystems.insert(
        eid(ecosystem),
        RawValue::from(json!({ "release": release.clone() })),
    );
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

/// Plan a single changed `core` module against the given `common` config
/// and per-run `overrides`, with `target` supplying its declared/published
/// state.
fn plan_core(
    common: CommonEcosystemConfig,
    target: FakeReleaseTarget,
    overrides: &BumpOverrides,
) -> ReleasePlan {
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(target);
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        overrides,
        &mut reporter,
    )
    .unwrap()
}

fn common_with_level(level: BumpLevel) -> CommonEcosystemConfig {
    let mut common = CommonEcosystemConfig::default();
    common.release.level = Some(level);
    common
}

/// Drive `release_plan` against one changed rust `core` module and return the
/// recorded event stream, so a test can assert the progressive decision
/// projection independently of the returned plan.
fn plan_events(common: CommonEcosystemConfig) -> Vec<toven_model::Event> {
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &BumpOverrides::default(),
        &mut reporter,
    )
    .unwrap();
    reporter.events().to_vec()
}

#[test]
fn release_plan_streams_a_decision_per_module_before_any_mutation() {
    let events = plan_events(common_with_level(BumpLevel::Minor));
    // The only work-item event `release plan` projects is the per-module
    // decision — never a staged/committed event, since planning mutates nothing.
    let decisions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            toven_model::Event::ModuleReleaseResolved {
                module,
                current_version,
                planned_version,
                level,
                ..
            } => Some((
                module.clone(),
                current_version.clone(),
                planned_version.clone(),
                level.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        decisions,
        vec![(
            "rust:core".to_string(),
            "0.1.0".to_string(),
            Some("0.2.0".to_string()),
            "minor".to_string(),
        )],
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, toven_model::Event::ModuleReleaseStaged { .. })),
        "planning must never emit a committed/staged event: {events:?}"
    );
}

#[test]
fn modules_are_examined_and_resolved_in_dependency_first_pairs() {
    let core = module("core", "crates/core");
    let app = module("app", "crates/app");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.edges = vec![Edge::new(app.id.clone(), core.id.clone(), DepKind::Normal)];
    response.modules = vec![core, app];

    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common_with_registry())
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core", "app"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let overrides = BumpOverrides::new();
    let mut reporter = RecordingReporter::new();
    release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &overrides,
        &mut reporter,
    )
    .unwrap();
    let events = reporter.events();

    let release_events: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|event| match event {
            toven_model::Event::ModuleReleaseExamining { module } => {
                Some(("examining", module.as_str()))
            }
            toven_model::Event::ModuleReleaseResolved { module, .. } => {
                Some(("resolved", module.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        release_events,
        vec![
            ("examining", "rust:core"),
            ("resolved", "rust:core"),
            ("examining", "rust:app"),
            ("resolved", "rust:app"),
        ],
        "release decisions must resolve in place and preserve dependency-first publication order: {events:?}"
    );
}

fn common_with_registry() -> CommonEcosystemConfig {
    CommonEcosystemConfig {
        release: ReleaseConfig {
            registry: Some("crates-io".into()),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    }
}

#[test]
fn go_registry_publication_is_rejected_during_release_resolution() {
    let module = module_for_ecosystem("go", "app", ".");
    let mut response = DiscoverResponse::new(eid("go"));
    response.modules = vec![module];
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            registry: Some("goproxy".into()),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid("go"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("go")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();

    let error = release_plan(
        &request,
        &document_for_ecosystem("go", &json!({ "registry": "goproxy" })),
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect_err("Go registry publication is invalid");

    assert!(error.to_string().contains("Go module"));
    assert!(error.to_string().contains("registry"));
}

/// Drive `release_plan` against one rust `core` module whose ecosystem
/// release config is `release`, returning the resolution result.
fn plan_with_release_config(release: ReleaseConfig) -> rskit_errors::AppResult<crate::ReleasePlan> {
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let common = CommonEcosystemConfig {
        release,
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
}

/// Drive `release_plan` against one rust `core` module whose ecosystem
/// release config is `release`, returning the resolution error.
fn plan_error_with_release_config(release: ReleaseConfig) -> rskit_errors::AppError {
    plan_with_release_config(release).expect_err("the configured setting must fail closed")
}

#[test]
fn private_visibility_to_a_public_only_registry_is_rejected() {
    // crates.io publishes every version world-readable, so a private/internal
    // release cannot target it — fail closed at plan time, before any tag or
    // push, rather than silently publishing it publicly.
    let error = plan_error_with_release_config(ReleaseConfig {
        registry: Some("crates-io".into()),
        visibility: Some(toven_ports::Visibility::Private),
        ..ReleaseConfig::default()
    });

    let message = error.to_string();
    assert!(message.contains("release.visibility"), "{message}");
    assert!(message.contains("public-only"), "{message}");
}

#[test]
fn private_visibility_on_a_tag_only_release_is_accepted() {
    // A private release that never publishes to a public registry (tag-only)
    // has no public-only mutation to conflict with, so it resolves cleanly.
    plan_with_release_config(ReleaseConfig {
        publish: Some(false),
        visibility: Some(toven_ports::Visibility::Private),
        ..ReleaseConfig::default()
    })
    .expect("a tag-only private release must be accepted");
}

#[test]
fn enabled_artifact_signing_is_accepted_now_that_signing_is_executable() {
    // Signing is a real capability (`toven release sign` via cosign), so
    // `sign.enabled = true` must no longer be rejected during planning —
    // the plan resolves and the release proceeds to the executable signer.
    let plan = plan_with_release_config(ReleaseConfig {
        sign: Some(toven_ports::SignConfig {
            enabled: true,
            signer: None,
            ..toven_ports::SignConfig::default()
        }),
        ..ReleaseConfig::default()
    })
    .expect("sign.enabled must be accepted");
    assert!(
        !plan.entries.is_empty(),
        "the release plan resolves with signing enabled"
    );
}

#[test]
fn branch_channel_mapping_cuts_a_prerelease_from_the_checked_out_branch() {
    // With a `next -> beta` mapping and the reader checked out on `next`,
    // a changed module cuts a `beta` prerelease with no `--pre` flag.
    let mut common = CommonEcosystemConfig::default();
    common.release.prerelease = Some(toven_ports::PrereleaseConfig {
        channels: vec!["beta".into()],
        branch_channels: std::collections::BTreeMap::from([("next".into(), "beta".into())]),
    });
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_current_branch("next")
        .with_tags(released_at_0_1_0(&["core"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .unwrap();
    assert_eq!(
        plan.entries[0].planned_version,
        Some(Version::parse("0.1.1-beta.1").unwrap())
    );
    assert_eq!(plan.entries[0].prerelease_channel.as_deref(), Some("beta"));
}

#[test]
fn branch_channel_mapping_is_stable_on_an_unmapped_branch() {
    // The same `next -> beta` mapping on the default `main` branch cuts a
    // stable release: only a mapped branch selects a prerelease channel.
    let mut common = CommonEcosystemConfig::default();
    common.release.prerelease = Some(toven_ports::PrereleaseConfig {
        channels: vec!["beta".into()],
        branch_channels: std::collections::BTreeMap::from([("next".into(), "beta".into())]),
    });
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_current_branch("main")
        .with_tags(released_at_0_1_0(&["core"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .unwrap();
    assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
    assert_eq!(plan.entries[0].prerelease_channel, None);
}

#[test]
fn token_env_on_a_registry_module_is_accepted_and_carried_to_the_plan_entry() {
    // `token_env` names the environment variable that holds the registry
    // token; the publishing adapter reads it at the toolchain boundary and
    // injects the credential, so a registry module configuring it plans
    // cleanly (no rejection) and the resolved name rides onto the entry for
    // publish-time injection.
    let mut common = CommonEcosystemConfig::default();
    common.release.registry = Some("crates-io".into());
    common.release.token_env = Some("CARGO_REGISTRY_TOKEN".into());
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect("a registry module with a configured token_env must plan cleanly");
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.entries[0].token_env.as_deref(),
        Some("CARGO_REGISTRY_TOKEN"),
        "the resolved token_env must ride onto the plan entry for publish-time injection"
    );
}

#[test]
fn token_env_on_a_tag_only_module_is_harmless_and_accepted() {
    // `token_env` only claims meaning for registry publication; a tag-only
    // module never publishes, so the inert field is not a safety risk.
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            token_env: Some("CARGO_REGISTRY_TOKEN".into()),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();

    release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect("a tag-only module's inert token_env must not fail the plan");
}

#[test]
fn required_changelog_rejects_a_missing_file() {
    let temp = rskit_fs::TempDir::new().expect("temp dir");
    let error = validate_required_changelogs(temp.path(), &changes_for("core"), &required_core())
        .expect_err("a missing required changelog must fail closed");

    assert!(error.to_string().contains("release.changelog.required"));
    assert!(error.to_string().contains("rust:core"));
}

#[test]
fn required_changelog_rejects_an_empty_unreleased_section() {
    let temp = rskit_fs::TempDir::new().expect("temp dir");
    std::fs::write(
        temp.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n### Added\n\n## [1.0.0]\n\n- Shipped\n",
    )
    .expect("write changelog");

    let error = validate_required_changelogs(temp.path(), &changes_for("core"), &required_core())
        .expect_err("an undocumented Unreleased section must fail closed");

    assert!(error.to_string().contains("Unreleased"));
    assert!(error.to_string().contains("rust:core"));
}

#[test]
fn required_changelog_accepts_a_documented_unreleased_section() {
    let temp = rskit_fs::TempDir::new().expect("temp dir");
    std::fs::write(
        temp.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Reworked the release planner\n",
    )
    .expect("write changelog");

    validate_required_changelogs(temp.path(), &changes_for("core"), &required_core())
        .expect("a documented Unreleased section satisfies the requirement");
}

#[test]
fn changelog_not_required_skips_the_file_check() {
    // No changelog file exists, but the module does not require one.
    let temp = rskit_fs::TempDir::new().expect("temp dir");
    let key = ModuleKey::bare(mref("core"));
    let resolved = ResolvedReleaseSettings::resolve(&ReleaseConfig::default(), None).unwrap();
    let settings = BTreeMap::from([(key, resolved)]);

    validate_required_changelogs(temp.path(), &changes_for("core"), &settings)
        .expect("an unrequired changelog is not read");
}

fn changes_for(name: &str) -> crate::versioning::change::ReleaseChanges {
    let mut changed = std::collections::BTreeSet::new();
    changed.insert(ModuleKey::bare(mref(name)));
    crate::versioning::change::ReleaseChanges {
        changed,
        records: BTreeMap::new(),
        commits: BTreeMap::new(),
        baselines: BTreeMap::new(),
    }
}

fn required_core() -> BTreeMap<ModuleKey, ResolvedReleaseSettings> {
    let mut config = CommonEcosystemConfig::default();
    config.release.changelog = Some(ChangelogConfig {
        path: None,
        required: true,
        roll: false,
    });
    let resolved = ResolvedReleaseSettings::resolve(&config.release, None).unwrap();
    BTreeMap::from([(ModuleKey::bare(mref("core")), resolved)])
}

#[test]
fn release_plan_bumps_changed_module_and_dependent_floor() {
    let core = module("core", "crates/core");
    let app = module("app", "crates/app");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core.clone(), app.clone()];
    response.edges = vec![Edge::new(app.id.clone(), core.id.clone(), DepKind::Normal)];

    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common_with_registry())
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("test"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core", "app"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let overrides = BumpOverrides::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &overrides,
        &mut reporter,
    )
    .unwrap();

    assert_eq!(plan.publish_count(), 2);
    assert_eq!(
        plan.entries[0].publication,
        PublicationPolicy::Registry {
            registry: "crates-io".into()
        }
    );
    assert_eq!(plan.entries[0].module, core.key());
    assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
    assert_eq!(plan.entries[0].reason, BumpReason::Changed);
    assert_eq!(plan.entries[0].winning_input, BumpSource::Default);
    assert_eq!(plan.entries[1].module, app.key());
    assert_eq!(plan.entries[1].reason, BumpReason::DependencyCascade);
    assert_eq!(plan.entries[1].cascade_origin, Some(core.key()));
    assert_eq!(
        plan.entries[1]
            .mutation
            .dep_floor_updates
            .get(&mref("core")),
        Some(&Version::new(0, 1, 1))
    );
}

#[test]
fn plan_and_bump_decide_identical_versions_from_the_single_path() {
    // `release plan` (a read-only Preview cut) and `release bump` (a mutating
    // cut) route their version decision through the one `plan_with_context`
    // -> `plan_entries` -> `toven_version::plan_bumps` path. Only the
    // `CutIntent` differs, so a changed module must plan the identical
    // version and floor cascade under both intents — the version decision has
    // exactly one owner.
    let core = module("core", "crates/core");
    let app = module("app", "crates/app");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.edges = vec![Edge::new(app.id.clone(), core.id.clone(), DepKind::Normal)];
    response.modules = vec![core, app];

    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common_with_registry())
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core", "app"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let overrides = BumpOverrides::new();

    let mut reporter = RecordingReporter::new();
    let preview = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &overrides,
        &mut reporter,
    )
    .unwrap();

    // Reprepare the front matter and drive the same shared cut with the
    // mutating `bump` intent.
    let document = document();
    let locator = PathDriverLocator::new();
    let mut bump_reporter = RecordingReporter::new();
    let context = prepare_front(
        &request.project_root,
        &document,
        &providers,
        &locator,
        &mut bump_reporter,
    )
    .unwrap();
    let targets = release_targets(&context, &readers).unwrap();
    let bumped = plan_with_context(
        &context,
        &request,
        &readers,
        &overrides,
        &targets,
        bump::CutIntent::Bump,
        &mut bump_reporter,
    )
    .unwrap();

    let planned = |plan: &ReleasePlan| {
        plan.entries
            .iter()
            .map(|entry| (entry.module.clone(), entry.planned_version.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        planned(&preview),
        planned(&bumped),
        "plan and bump must decide identical versions from the single path"
    );
    assert_eq!(
        preview.entries[0].planned_version,
        Some(Version::new(0, 1, 1))
    );
}

#[test]
fn release_plan_cascades_transitively_to_indirect_dependents() {
    let core = module("core", "crates/core");
    let mid = module("mid", "crates/mid");
    let top = module("top", "crates/top");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core.clone(), mid.clone(), top.clone()];
    response.edges = vec![
        Edge::new(mid.id.clone(), core.id.clone(), DepKind::Normal),
        Edge::new(top.id.clone(), mid.id, DepKind::Normal),
    ];

    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common_with_registry())
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("test"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core", "mid", "top"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let overrides = BumpOverrides::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &overrides,
        &mut reporter,
    )
    .unwrap();

    // core changed → mid (direct) and top (transitive) both cascade, and both
    // report the changed root as the cascade origin.
    assert_eq!(plan.publish_count(), 3);
    assert_eq!(plan.entries[2].module, top.key());
    assert_eq!(plan.entries[2].reason, BumpReason::DependencyCascade);
    assert_eq!(plan.entries[2].cascade_origin, Some(core.key()));
    assert_eq!(plan.entries[2].planned_version, Some(Version::new(0, 1, 1)));
    assert_eq!(
        plan.entries[2].mutation.dep_floor_updates.get(&mref("mid")),
        Some(&Version::new(0, 1, 1))
    );
}

#[test]
fn release_plan_generates_grouped_attributed_changelog_from_commits() {
    let core = module("core", "crates/core");
    let app = module("app", "crates/app");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core.clone(), app.clone()];

    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("test"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core", "app"]))
        .with_changed_since(vec![
            ChangeRecord::new("crates/core/src/lib.rs", ChangeStatus::Modified),
            ChangeRecord::new("crates/app/src/lib.rs", ChangeStatus::Modified),
        ])
        .with_worktree_status(vec![ChangeRecord::new(
            "crates/app/src/main.rs",
            ChangeStatus::Modified,
        )])
        .with_commits_since(vec![
            toven_ports::CommitSummary::new("abc123def456", "feat(core): add widget")
                .with_author("Ada", "42+ada@users.noreply.github.com"),
            toven_ports::CommitSummary::new("def456abc789", "fix: correct off-by-one")
                .with_author("Bo", "bo@example.com"),
        ]);
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let overrides = BumpOverrides::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &overrides,
        &mut reporter,
    )
    .unwrap();
    let by_module = plan
        .entries
        .iter()
        .map(|entry| (entry.module.clone(), entry.changelog.lines.clone()))
        .collect::<BTreeMap<_, _>>();

    // The scripted commits are rendered as grouped, attributed Keep a
    // Changelog bullets — the GitHub noreply email becomes an `@handle`, the
    // non-GitHub author falls back to a name, and each commit lands under its
    // Conventional Commit group heading.
    let core_lines = by_module.get(&core.key()).unwrap();
    assert_eq!(
        core_lines,
        &vec![
            "### Added".to_string(),
            "- **core**: add widget — by @ada (abc123def456)".to_string(),
            String::new(),
            "### Fixed".to_string(),
            "- correct off-by-one — by Bo (def456abc789)".to_string(),
        ]
    );
    // The double returns the same commits for every module (path scoping is
    // the adapter's job, covered by the real-repo `commits_since` test), so
    // `app` renders the same grouped changelog.
    assert_eq!(by_module.get(&app.key()).unwrap(), core_lines);
}

#[test]
fn config_minor_level_bumps_minor() {
    let plan = plan_core(
        common_with_level(BumpLevel::Minor),
        FakeReleaseTarget::new(),
        &BumpOverrides::new(),
    );
    assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 2, 0)));
    assert_eq!(plan.entries[0].level, BumpLevel::Minor);
    assert_eq!(plan.entries[0].winning_input, BumpSource::Config);
}

#[test]
fn argv_level_override_beats_config() {
    let overrides = BumpOverrides::new()
        .with_module_level(mref("core"), BumpLevel::Major)
        .unwrap();
    let plan = plan_core(
        common_with_level(BumpLevel::Minor),
        FakeReleaseTarget::new(),
        &overrides,
    );
    assert_eq!(plan.entries[0].planned_version, Some(Version::new(1, 0, 0)));
    assert_eq!(plan.entries[0].level, BumpLevel::Major);
    assert_eq!(plan.entries[0].winning_input, BumpSource::Argv);
}

#[test]
fn set_version_pins_an_explicit_target() {
    let overrides = BumpOverrides::new()
        .with_set_version(mref("core"), Version::new(3, 1, 4))
        .unwrap();
    let plan = plan_core(
        CommonEcosystemConfig::default(),
        FakeReleaseTarget::new(),
        &overrides,
    );
    assert_eq!(plan.entries[0].planned_version, Some(Version::new(3, 1, 4)));
    assert_eq!(plan.entries[0].reason, BumpReason::Explicit);
    assert_eq!(plan.entries[0].winning_input, BumpSource::SetVersion);
}

#[test]
fn pre_channel_cuts_a_prerelease() {
    let mut common = CommonEcosystemConfig::default();
    common.release.prerelease = Some(PrereleaseConfig {
        channels: vec!["rc".to_string()],
        ..PrereleaseConfig::default()
    });
    let overrides = BumpOverrides::new().with_prerelease("rc");
    let plan = plan_core(common, FakeReleaseTarget::new(), &overrides);
    assert_eq!(
        plan.entries[0].planned_version,
        Some(Version::parse("0.1.1-rc.1").unwrap())
    );
    assert_eq!(plan.entries[0].prerelease_channel.as_deref(), Some("rc"));
}

#[test]
fn unrecognized_pre_channel_is_rejected() {
    let overrides = BumpOverrides::new().with_prerelease("nightly");
    let core = module("core", "crates/core");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core];
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    assert!(
        release_plan(
            &request,
            &document(),
            &providers,
            &readers,
            &overrides,
            &mut reporter,
        )
        .is_err()
    );
}

#[test]
fn module_at_registry_max_is_a_reported_no_op() {
    let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);
    let plan = plan_core(common_with_registry(), target, &BumpOverrides::new());
    // Patch of 0.1.0 → 0.1.1 which the registry already has: up to date.
    assert_eq!(plan.entries[0].planned_version, Some(Version::new(0, 1, 1)));
    assert!(plan.entries[0].up_to_date);
    assert!(!plan.entries[0].publish_needed);
    assert_eq!(plan.publish_count(), 0);
}

#[test]
fn offline_skips_the_registry_and_still_publishes() {
    // Registry already reports 0.1.1, but --offline ignores the registry, so
    // idempotency is not anchored on it and a publish is still proposed.
    let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);
    let overrides = BumpOverrides::new().with_offline(true);
    let plan = plan_core(common_with_registry(), target, &overrides);
    assert!(!plan.entries[0].up_to_date);
    assert!(plan.entries[0].publish_needed);
}

#[test]
fn default_publication_policy_is_tag_only() {
    let plan = plan_core(
        CommonEcosystemConfig::default(),
        FakeReleaseTarget::new(),
        &BumpOverrides::new(),
    );

    assert!(!plan.entries[0].publish_needed);
    assert_eq!(plan.entries[0].publication, PublicationPolicy::TagOnly);
}

#[test]
fn dependent_upgrade_raises_floor_without_own_bump() {
    let core = module("core", "crates/core");
    let app = module("app", "crates/app");
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![core.clone(), app.clone()];
    response.edges = vec![Edge::new(app.id.clone(), core.id, DepKind::Normal)];

    let mut common = CommonEcosystemConfig::default();
    common.release.dependent_version = Some(DependentVersion::Upgrade);
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let request = PlanRequest::new(
        "r1",
        "t",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
    .with_selection(Selection::Changed(Some(BaselineSpec::explicit("main"))));
    let vcs = FakeVcsReader::new()
        .with_tags(released_at_0_1_0(&["core", "app"]))
        .with_changed_since(vec![ChangeRecord::new(
            "crates/core/src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let readers = MemberVcsReaders::single(&vcs, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let overrides = BumpOverrides::new();
    let plan = release_plan(
        &request,
        &document(),
        &providers,
        &readers,
        &overrides,
        &mut reporter,
    )
    .unwrap();

    let app_entry = plan
        .entries
        .iter()
        .find(|entry| entry.module == app.key())
        .expect("app entry");
    assert_eq!(app_entry.planned_version, None);
    assert_eq!(app_entry.planned_tag, None);
    assert!(!app_entry.publish_needed);
    assert_eq!(
        app_entry.mutation.dep_floor_updates.get(&mref("core")),
        Some(&Version::new(0, 1, 1))
    );
}

#[test]
fn planned_entries_carry_the_release_tag_name() {
    // The plan must explain the exact tag a mutating run would create.
    let plan = plan_core(
        common_with_level(BumpLevel::Minor),
        FakeReleaseTarget::new(),
        &BumpOverrides::new(),
    );

    let entry = plan.entries.first().expect("one planned entry");
    assert_eq!(entry.planned_version, Some(Version::new(0, 2, 0)));
    assert_eq!(entry.planned_tag.as_deref(), Some("rust/core@0.2.0"));
}

#[test]
fn reconcile_policy_defaults_to_semver_cascade_when_no_modules() {
    let empty = BTreeMap::new();
    assert_eq!(reconcile_policy(&empty).unwrap(), BumpPolicy::SemverCascade);
}

#[test]
fn resolved_settings_carry_the_single_policy() {
    let settings =
        ResolvedReleaseSettings::resolve(&toven_ports::ReleaseConfig::default(), None).unwrap();
    assert_eq!(settings.policy, BumpPolicy::SemverCascade);
}
