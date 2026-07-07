//! Explicit selection: resolve user-named [`ModuleSelector`] targets to seeds.

use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult};
use rskit_util::strings::{Ambiguity, resolve_unique};
use toven_model::{Graph, Module, ModuleKey, ModuleRef, ModuleSelector, NamePattern};

use crate::plan::discover::Federation;

/// Resolve the user-named [`ModuleSelector`] targets to seed module keys.
///
/// Each selector is matched against the discovered graph: a bare name resolves
/// across every ecosystem, an ecosystem/workspace-qualified name scopes the
/// match, a glob expands to its explicit set, and a whole-workspace pattern
/// activates every module its matching workspaces own. Canonical `ecosystem:name`
/// identity is unchanged — only the accepted *input* is relaxed.
///
/// # Errors
/// - A target that resolves to no discovered module is an
///   [`AppError::invalid_input`] listing the available identities — Toven never
///   silently plans an empty run.
/// - A bare exact name matching modules in more than one ecosystem is a distinct
///   ambiguity error naming the qualified candidates; the user must qualify it. A
///   glob is an explicit set, so multiple matches are the intent, never an error.
pub(super) fn explicit_seeds(
    targets: &[ModuleSelector],
    graph: &Graph,
    federation: &Federation,
) -> AppResult<BTreeSet<ModuleKey>> {
    let mut seeds = BTreeSet::new();
    for target in targets {
        match target {
            ModuleSelector::Name(pattern) => {
                seeds.extend(resolve_name(pattern, graph)?);
            }
            ModuleSelector::Ecosystem { ecosystem, name } => {
                let matches = matching_keys(graph, |key| {
                    key.module().ecosystem == *ecosystem && name.matches(&key.module().name)
                });
                if matches.is_empty() {
                    return Err(unknown_module_error(target, graph));
                }
                seeds.extend(matches);
            }
            ModuleSelector::Workspace { workspace, name } => {
                let matches: Vec<ModuleKey> = federation
                    .modules
                    .iter()
                    .filter(|module| {
                        module.workspace.as_ref() == Some(workspace)
                            && name.matches(&module.id.name)
                    })
                    .map(Module::key)
                    .collect();
                if matches.is_empty() {
                    return Err(unknown_module_error(target, graph));
                }
                seeds.extend(matches);
            }
            ModuleSelector::WholeWorkspace(pattern) => {
                let matches: Vec<ModuleKey> = federation
                    .modules
                    .iter()
                    .filter(|module| {
                        module
                            .workspace
                            .as_ref()
                            .is_some_and(|id| pattern.matches(id.as_str()))
                    })
                    .map(Module::key)
                    .collect();
                if matches.is_empty() {
                    return Err(unknown_workspace_error(pattern, federation));
                }
                seeds.extend(matches);
            }
            _ => return Err(unsupported_selector_error(target)),
        }
    }
    Ok(seeds)
}

/// Resolve a bare [`Name`](ModuleSelector::Name) pattern to its module keys.
///
/// A glob is an explicit set (multiple matches are intended); a bare exact name
/// must resolve to a single ecosystem, so matches spanning two ecosystems are a
/// typed ambiguity error rather than a silent union.
fn resolve_name(pattern: &NamePattern, graph: &Graph) -> AppResult<Vec<ModuleKey>> {
    match pattern {
        NamePattern::Glob(_) => {
            let matches = matching_keys(graph, |key| pattern.matches(&key.module().name));
            if matches.is_empty() {
                Err(unknown_name_error(pattern, graph))
            } else {
                Ok(matches)
            }
        }
        NamePattern::Exact(name) => {
            let mut refs: Vec<ModuleRef> = graph
                .modules()
                .map(|module| module.id.clone())
                .filter(|reference| reference.name == *name)
                .collect();
            refs.sort();
            refs.dedup();
            let resolved = resolve_unique(name, refs, |reference| reference.name.as_str())
                .map_err(|ambiguity| ambiguous_name_error(&ambiguity))?;
            resolved.map_or_else(
                || Err(unknown_name_error(pattern, graph)),
                |reference| Ok(matching_keys(graph, |key| key.module() == &reference)),
            )
        }
    }
}

/// Every module key satisfying `keep`, in graph order.
fn matching_keys(graph: &Graph, keep: impl Fn(&ModuleKey) -> bool) -> Vec<ModuleKey> {
    graph
        .modules()
        .map(Module::key)
        .filter(|key| keep(key))
        .collect()
}

/// Sorted, de-duplicated canonical `ecosystem:name` identities in the graph.
fn available_modules(graph: &Graph) -> Vec<String> {
    let mut available: Vec<String> = graph
        .modules()
        .map(|module| module.key().module().to_string())
        .collect();
    available.sort();
    available.dedup();
    available
}

/// Typed error for a forward-compatible selector variant not yet handled.
fn unsupported_selector_error(selector: &ModuleSelector) -> AppError {
    AppError::invalid_input("module", format!("unsupported selector '{selector}'"))
}

/// Typed error for a bare selector that matches no discovered module.
fn unknown_name_error(pattern: &NamePattern, graph: &Graph) -> AppError {
    AppError::invalid_input(
        "module",
        format!(
            "no module matches '{pattern}'; discovered modules: {}",
            available_modules(graph).join(", ")
        ),
    )
}

/// Typed error for a qualified selector that matches no discovered module.
fn unknown_module_error(selector: &ModuleSelector, graph: &Graph) -> AppError {
    AppError::invalid_input(
        "module",
        format!(
            "no module matches '{selector}'; discovered modules: {}",
            available_modules(graph).join(", ")
        ),
    )
}

/// Typed ambiguity error for a bare exact name matching multiple ecosystems.
fn ambiguous_name_error(ambiguity: &Ambiguity<ModuleRef>) -> AppError {
    let candidates: Vec<String> = ambiguity.matches.iter().map(ModuleRef::to_string).collect();
    let hint = candidates.first().cloned().unwrap_or_default();
    AppError::invalid_input(
        "module",
        format!(
            "'{}' is ambiguous; matched {} — qualify it (e.g. {hint})",
            ambiguity.input,
            candidates.join(", ")
        ),
    )
}

/// Typed error for a `--workspace` pattern that owns no discovered module.
fn unknown_workspace_error(pattern: &NamePattern, federation: &Federation) -> AppError {
    let mut available: Vec<String> = federation
        .workspaces
        .iter()
        .map(|workspace| workspace.id.to_string())
        .collect();
    available.sort();
    available.dedup();
    AppError::invalid_input(
        "workspace",
        format!(
            "no workspace matches '{pattern}'; discovered workspaces: {}",
            available.join(", ")
        ),
    )
}
