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
use toven_model::{Module, ModuleKey};
use toven_ports::{Provider, Reporter};

use super::aggregate::{CoverageInputs, aggregate};
use super::read::{COVERAGE_DIR, read_profiles};
use super::report::CoverageReport;
use super::settings::{CoverageOverrides, ResolvedCoverageSettings};
use toven_engine_core::config::Document;
use toven_engine_core::federation::baseline::MemberVcsReaders;
use toven_engine_core::federation::resolve::PathDriverLocator;
use toven_engine_core::plan::affected::{active_modules, changed_for_members};
use toven_engine_core::plan::{PlanRequest, Selection, prepare_front};

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

    for (_, ecosystem, adapter) in context.adapters.iter() {
        adapter
            .common()
            .coverage
            .validate(&format!("ecosystems.{ecosystem}.coverage"))?;
    }

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
    let profiles = read_profiles(&request.project_root.as_path().join(COVERAGE_DIR))?;

    Ok(aggregate(&CoverageInputs {
        project_root: request.project_root.as_path(),
        modules: &scope,
        profiles: &profiles,
        settings: &settings,
        changed: changed.as_ref(),
    }))
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
