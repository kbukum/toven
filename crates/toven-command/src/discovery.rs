//! Command discovery: normalize the **declared** module/edge set into the
//! federated graph. No tooling probe, no filesystem walk, no inference — the
//! escape-hatch adapter only reflects what the user declared.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{
    DepKind, EcosystemId, Edge, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverRequest, DiscoverResponse};

use crate::config::{CommandConfig, DeclaredModule};

/// The command driver name stamped on the workspace and resource groups.
const COMMAND_TOOL: &str = "command";

/// Normalize the declared modules and `depends_on` edges into a
/// [`DiscoverResponse`] with a single `command` workspace.
pub(crate) fn discover(
    config: &CommandConfig,
    request: &DiscoverRequest,
) -> AppResult<DiscoverResponse> {
    let ecosystem = command_id()?;
    let workspace_id = WorkspaceId::new(COMMAND_TOOL)?;

    let mut modules: BTreeMap<ModuleRef, Module> = BTreeMap::new();
    let mut declared_names: BTreeSet<String> = BTreeSet::new();

    for declared in &config.modules {
        let id = ModuleRef::new(ecosystem.clone(), declared.name.as_str())?;
        if modules.contains_key(&id) {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!("duplicate command module '{id}': module names must be unique"),
            ));
        }
        declared_names.insert(declared.name.clone());
        modules.insert(id.clone(), build_module(&id, &workspace_id, declared)?);
    }

    let edges = build_edges(&ecosystem, &config.modules, &declared_names)?;

    let workspace = Workspace::new(
        workspace_id,
        RepoPath::new(".")?,
        ToolchainTag::new(COMMAND_TOOL),
    );

    let mut response = DiscoverResponse::new(ecosystem);
    response.schema_version = request.schema_version;
    response.workspaces = vec![workspace];
    response.modules = modules.into_values().collect();
    response.edges = edges;
    Ok(response)
}

/// The canonical command ecosystem id.
fn command_id() -> AppResult<EcosystemId> {
    EcosystemId::new(COMMAND_TOOL)
}

/// Build one module from its declaration, stamping a per-module resource group.
fn build_module(
    id: &ModuleRef,
    workspace_id: &WorkspaceId,
    declared: &DeclaredModule,
) -> AppResult<Module> {
    let root = RepoPath::new(declared.root.as_str())?;
    let mut module = Module::new(id.clone(), root);
    module.workspace = Some(workspace_id.clone());
    if let Some(manifest) = &declared.manifest {
        module.manifest = Some(RepoPath::new(manifest.as_str())?);
    }
    // Each declared module serializes on its own resource group by default; the
    // adapter infers no shared contention between independent commands.
    module.resource_group = Some(format!("{COMMAND_TOOL}:{}", declared.name));
    Ok(module)
}

/// Turn each module's declared `depends_on` into intra-ecosystem edges, rejecting
/// a dependency on an undeclared module (a typo at the trust boundary).
fn build_edges(
    ecosystem: &EcosystemId,
    declared: &[DeclaredModule],
    declared_names: &BTreeSet<String>,
) -> AppResult<Vec<Edge>> {
    let mut edges: BTreeSet<Edge> = BTreeSet::new();
    for module in declared {
        let from = ModuleRef::new(ecosystem.clone(), module.name.as_str())?;
        for dependency in &module.depends_on {
            if !declared_names.contains(dependency) {
                return Err(AppError::invalid_input(
                    format!("ecosystems.command.modules.{}.depends_on", module.name),
                    format!("'{dependency}' is not a declared command module"),
                ));
            }
            let to = ModuleRef::new(ecosystem.clone(), dependency.as_str())?;
            if from != to {
                edges.insert(Edge::new(from.clone(), to, DepKind::Normal));
            }
        }
    }
    Ok(edges.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use toven_model::EcosystemId;

    use super::{build_edges, command_id};
    use crate::config::DeclaredModule;

    fn declared(name: &str, depends_on: &[&str]) -> DeclaredModule {
        DeclaredModule {
            name: name.to_string(),
            root: name.to_string(),
            manifest: None,
            depends_on: depends_on.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn command_id_is_canonical() {
        assert_eq!(command_id().unwrap(), EcosystemId::new("command").unwrap());
    }

    #[test]
    fn declared_depends_on_becomes_an_edge() {
        let modules = vec![declared("site", &["api"]), declared("api", &[])];
        let names: BTreeSet<String> = modules.iter().map(|m| m.name.clone()).collect();
        let edges = build_edges(&command_id().unwrap(), &modules, &names).expect("edges");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from.name, "site");
        assert_eq!(edges[0].to.name, "api");
    }

    #[test]
    fn depends_on_unknown_module_is_rejected() {
        let modules = vec![declared("site", &["ghost"])];
        let names: BTreeSet<String> = modules.iter().map(|m| m.name.clone()).collect();
        let error = build_edges(&command_id().unwrap(), &modules, &names)
            .expect_err("unknown dependency rejected");
        assert!(error.to_string().contains("not a declared"), "{error}");
    }
}
