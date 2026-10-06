//! The `toven coverage` engine entry: run-agnostic aggregation of the emitted
//! coverage profiles into a gated [`CoverageReport`].
//!
//! Read-only over an already-run coverage task: the CLI verb runs the
//! recognized coverage task (emitting profiles into
//! [`COVERAGE_DIR`](super::read::COVERAGE_DIR)), then calls this to attribute
//! the profiles to modules, fold each module's metrics, and gate them against
//! the resolved `[…coverage]` thresholds. The measurement is the ecosystem
//! tool's job; the aggregation and pass/fail verdict are Toven's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rskit_errors::AppResult;
use toven_model::{AbsPath, Module, ModuleKey};
use toven_ports::{Provider, Reporter};

use super::aggregate::{CoverageInputs, aggregate};
use super::goimport::GoImportRoots;
use super::read::{COVERAGE_DIR, read_profiles};
use super::report::CoverageReport;
use super::settings::{CoverageOverrides, ResolvedCoverageSettings};
use super::stream::emit_verdicts;
use toven_core::config::Document;
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::affected::{active_modules, changed_for_members};
use toven_core::plan::{PlanContext, PlanRequest, Selection, prepare_front};

/// Aggregate and gate the coverage profiles emitted for `request`'s scope.
///
/// Resolves each in-scope module's coverage settings (ecosystem default →
/// profile → per-module override → argv `overrides`), reads the profiles staged
/// under [`COVERAGE_DIR`], and gates them. Under a changed selection the scope
/// narrows to the affected modules and the `changed_line` floor applies to the
/// changed files.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, VCS I/O failures, an
/// invalid ecosystem coverage config, and a profile read/parse error.
pub fn coverage_report(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
    overrides: &CoverageOverrides,
) -> AppResult<CoverageReport> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;

    validate_settings(&context)?;

    let active = active_modules(request, &context.graph, &context.federation, readers)?;
    let scope: Vec<Module> = context
        .federation
        .modules
        .iter()
        .filter(|module| active.modules.contains(&module.key()))
        .cloned()
        .collect();

    let mut settings: BTreeMap<ModuleKey, ResolvedCoverageSettings> = BTreeMap::new();
    for module in &scope {
        let Some(adapter) = context
            .adapters
            .get(module.member.as_ref(), &module.id.ecosystem)
        else {
            continue;
        };
        let ecosystem = &adapter.common().coverage;
        let over = document
            .modules
            .get(&module.id.to_string())
            .map(|entry| &entry.coverage);
        settings.insert(
            module.key(),
            ResolvedCoverageSettings::resolve(ecosystem, &module.id.name, over)
                .with_overrides(overrides),
        );
    }

    let changed = changed_files(request, readers)?;
    // Map Go import paths and attribute files against every discovered module,
    // not just the active scope, so an unselected module's files land with
    // their real owner and are dropped instead of crediting an active parent.
    let go_roots = GoImportRoots::from_modules(&context.federation.modules);
    let profiles = read_profiles(
        &request.project_root.as_path().join(COVERAGE_DIR),
        &go_roots,
    )?;

    let report = aggregate(&CoverageInputs {
        project_root: request.project_root.as_path(),
        owners: &context.federation.modules,
        modules: &scope,
        profiles: &profiles,
        settings: &settings,
        changed: changed.as_ref(),
    });

    // Emit each module's settled verdict as the aggregation completes, so the
    // CLI's live reporter reports coverage per module rather than one terminal
    // table. The exit stays derived from the returned report's summary.
    emit_verdicts(reporter, &report)?;

    Ok(report)
}

/// Check every ecosystem's coverage config against the discovered modules,
/// without running anything.
///
/// The CLI calls this before the coverage task, so a typo in `exclude` or a
/// profile's `modules` fails fast instead of after a full measurement.
/// [`coverage_report`] repeats the check over its own discovery.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, an invalid ecosystem
/// coverage config, and an `exclude`/profile entry naming no discovered module.
pub fn validate_coverage_config(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    reporter: &mut dyn Reporter,
) -> AppResult<()> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(project_root, document, providers, &locator, reporter)?;
    validate_settings(&context)
}

/// Validate each adapter's coverage block and its module names against the
/// modules that adapter discovered.
fn validate_settings(context: &PlanContext) -> AppResult<()> {
    for (member, ecosystem, adapter) in context.adapters.iter() {
        let field = format!("ecosystems.{ecosystem}.coverage");
        let coverage = &adapter.common().coverage;
        coverage.validate(&field)?;
        let known: BTreeSet<&str> = context
            .federation
            .modules
            .iter()
            .filter(|module| module.member.as_ref() == member && &module.id.ecosystem == ecosystem)
            .map(|module| module.id.name.as_str())
            .collect();
        coverage.validate_module_names(&field, &known)?;
    }
    Ok(())
}

/// The changed-file set (workspace-relative) under a changed selection; `None`
/// for a whole-scope run, which never gates `changed_line`.
fn changed_files(
    request: &PlanRequest,
    readers: &MemberVcsReaders<'_>,
) -> AppResult<Option<BTreeSet<PathBuf>>> {
    match &request.selection {
        Selection::Changed(spec) => Ok(Some(
            changed_for_members(readers, spec.as_ref())?
                .into_iter()
                .map(|record| record.path)
                .collect(),
        )),
        Selection::ChangedPaths(paths) => Ok(Some(paths.iter().map(PathBuf::from).collect())),
        // `Selection` is `#[non_exhaustive]`; whole-scope selections (`All`,
        // `Explicit`, and any future variant) never gate `changed_line`.
        _ => Ok(None),
    }
}
