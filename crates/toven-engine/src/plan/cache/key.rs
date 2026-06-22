//! The per-unit content cache key (blake3).
//!
//! A unit's key folds the module `source_hash`, a recursive `dep_hash` over the
//! transitive (cross-ecosystem) dependency closure, the rendered base argv
//! (`task_hash`), the `shared_inputs` file hashes, and the phase-6 toolchain
//! identity; rendered passthrough args are folded only when `cache_args` is set.
//!
//! Because the key folds dependencies' **source** hashes (not build outputs),
//! every verdict is determinable statically in PLAN and a changed leaf naturally
//! re-keys its dependents.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use toven_model::{Graph, ModuleRef};

use super::super::source::SourceDigest;

/// Content identities for every module source tree, keyed by module ref.
pub(in crate::plan) type SourceHashes = BTreeMap<ModuleRef, String>;

/// Hash every module's source tree once, up front, for reuse across unit keys.
///
/// # Errors
/// Propagates a [`SourceDigest`] read failure.
pub(in crate::plan) fn source_hashes(
    modules: &[toven_model::Module],
    digest: &dyn SourceDigest,
) -> AppResult<SourceHashes> {
    let mut hashes = SourceHashes::new();
    for module in modules {
        hashes.insert(module.id.clone(), digest.module(module)?);
    }
    Ok(hashes)
}

/// The per-unit inputs that compose its content key.
#[derive(Debug, Clone, Copy)]
pub(in crate::plan) struct KeyInputs<'a> {
    /// Module the unit operates on.
    pub(in crate::plan) module: &'a ModuleRef,
    /// Rendered base argv (without passthrough) — the `task_hash` source.
    pub(in crate::plan) base_argv: &'a [String],
    /// Workspace-relative shared-input paths folded into the key.
    pub(in crate::plan) shared_inputs: &'a [String],
    /// Opaque toolchain identity (`tool@version`) for the owning workspace.
    pub(in crate::plan) toolchain_identity: &'a str,
    /// Whether rendered passthrough args enter the key.
    pub(in crate::plan) cache_args: bool,
    /// User passthrough args (folded only when `cache_args`).
    pub(in crate::plan) passthrough: &'a [String],
}

/// A forward adjacency map (`from` → its direct dependency `to`s), built once and
/// reused across every unit's closure to avoid rescanning all edges per node.
pub(in crate::plan) type Adjacency = BTreeMap<ModuleRef, Vec<ModuleRef>>;

/// Build the forward adjacency map of the graph in one pass.
pub(in crate::plan) fn forward_adjacency(graph: &Graph) -> Adjacency {
    let mut adjacency: Adjacency = BTreeMap::new();
    for edge in graph.edges() {
        adjacency
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    adjacency
}

/// Derive the blake3 content key for one unit.
///
/// # Errors
/// A missing module/dependency source hash (internal inconsistency) or a
/// [`SourceDigest`] read failure while hashing shared inputs.
pub(in crate::plan) fn unit_key(
    inputs: &KeyInputs,
    adjacency: &Adjacency,
    sources: &SourceHashes,
    digest: &dyn SourceDigest,
) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();

    let module_hash = source_hash(sources, inputs.module)?;
    fold(&mut hasher, b"module", module_hash.as_bytes());

    for dependency in transitive_dependencies(inputs.module, adjacency) {
        let dep_hash = source_hash(sources, &dependency)?;
        fold(&mut hasher, b"dep", dependency.to_string().as_bytes());
        fold(&mut hasher, b"dep-hash", dep_hash.as_bytes());
    }

    for arg in inputs.base_argv {
        fold(&mut hasher, b"argv", arg.as_bytes());
    }

    for path in inputs.shared_inputs {
        let hash = digest.path(Path::new(path))?;
        fold(&mut hasher, b"shared", path.as_bytes());
        fold(&mut hasher, b"shared-hash", hash.as_bytes());
    }

    fold(
        &mut hasher,
        b"toolchain",
        inputs.toolchain_identity.as_bytes(),
    );

    if inputs.cache_args {
        for arg in inputs.passthrough {
            fold(&mut hasher, b"args", arg.as_bytes());
        }
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Fold one labelled field into the hasher with unambiguous framing.
fn fold(hasher: &mut blake3::Hasher, label: &[u8], value: &[u8]) {
    hasher.update(label);
    hasher.update(b":");
    hasher.update(value);
    hasher.update(b"\0");
}

/// Look up a module's precomputed source hash, erroring if it is absent.
///
/// Every graph module is hashed up front by [`source_hashes`], so a missing
/// entry signals an internal inconsistency; hashing an empty fallback would let
/// distinct graphs alias to one key, so this is a hard error rather than a
/// silent default.
fn source_hash<'a>(sources: &'a SourceHashes, module: &ModuleRef) -> AppResult<&'a String> {
    sources.get(module).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("missing source hash for module '{module}'"),
        )
    })
}

/// The transitive set of modules `module` depends on, via the forward adjacency.
fn transitive_dependencies(module: &ModuleRef, adjacency: &Adjacency) -> BTreeSet<ModuleRef> {
    let mut dependencies = BTreeSet::new();
    let mut pending = vec![module.clone()];
    while let Some(current) = pending.pop() {
        let Some(neighbors) = adjacency.get(&current) else {
            continue;
        };
        for next in neighbors {
            if dependencies.insert(next.clone()) {
                pending.push(next.clone());
            }
        }
    }
    dependencies
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rskit_errors::AppResult;
    use toven_model::{DepKind, EcosystemId, Edge, Graph, Module, ModuleRef, RepoPath};

    use super::super::super::source::SourceDigest;
    use super::{KeyInputs, SourceHashes, forward_adjacency, unit_key};

    struct NoFileDigest;
    impl SourceDigest for NoFileDigest {
        fn module(&self, _module: &Module) -> AppResult<String> {
            Ok(String::new())
        }
        fn path(&self, _repo_relative: &Path) -> AppResult<String> {
            Ok(String::new())
        }
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    fn graph() -> Graph {
        // app depends on errors.
        Graph::build(
            vec![
                Module::new(mref("app"), RepoPath::new("app").unwrap()),
                Module::new(mref("errors"), RepoPath::new("errors").unwrap()),
            ],
            vec![Edge::new(mref("app"), mref("errors"), DepKind::Normal)],
        )
        .unwrap()
    }

    fn app_key(errors_hash: &str) -> String {
        let app = mref("app");
        let mut sources = SourceHashes::new();
        sources.insert(mref("app"), "app-1".to_string());
        sources.insert(mref("errors"), errors_hash.to_string());
        let inputs = KeyInputs {
            module: &app,
            base_argv: &["cargo".to_string(), "test".to_string()],
            shared_inputs: &[],
            toolchain_identity: "cargo@1",
            cache_args: false,
            passthrough: &[],
        };
        unit_key(
            &inputs,
            &forward_adjacency(&graph()),
            &sources,
            &NoFileDigest,
        )
        .unwrap()
    }

    #[test]
    fn changed_leaf_rekeys_its_dependent() {
        // A dependency's source hash folds into the dependent's key, so changing
        // the leaf re-keys app; an unchanged leaf reproduces the same key.
        assert_eq!(app_key("errors-1"), app_key("errors-1"));
        assert_ne!(app_key("errors-1"), app_key("errors-2"));
    }
}
