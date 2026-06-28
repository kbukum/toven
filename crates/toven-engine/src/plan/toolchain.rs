//! Toolchain: resolve `{tool, version}` once per active workspace.
//!
//! A workspace is *active* when it owns ≥1 active module. The engine probes each
//! such workspace's toolchain exactly once (untouched ecosystems are never
//! probed) and stamps the resolved version onto its [`ToolchainTag`]; a
//! needed-but-failing probe is a hard PLAN error. Probing is an injected port so
//! the planner stays pure and tests substitute a deterministic prober.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_process::{CapturedIo, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_model::{
    AbsPath, EcosystemId, MemberId, ModuleKey, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{ToolchainProbe, ToolchainProber};

use super::configure::MemberAdapters;
use super::discover::Federation;

/// Default per-probe timeout: bounded so a hung toolchain cannot stall PLAN.
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on captured probe output (64 KiB) — a version line is tiny.
const MAX_PROBE_OUTPUT_BYTES: usize = 64 * 1024;

/// The production [`ToolchainProber`]: a captured, bounded, timed subprocess.
#[derive(Debug, Clone)]
pub struct ProcessToolchainProber {
    timeout: Duration,
    max_output_bytes: usize,
}

impl Default for ProcessToolchainProber {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_PROBE_TIMEOUT,
            max_output_bytes: MAX_PROBE_OUTPUT_BYTES,
        }
    }
}

impl ProcessToolchainProber {
    /// Construct a prober with the default timeout and output cap.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ToolchainProber for ProcessToolchainProber {
    fn probe(&self, probe: &ToolchainProbe, workspace_root: &Path) -> AppResult<String> {
        let spec = ProcessSpec::new(&probe.program)
            .args(probe.args.clone())
            .dir(workspace_root);
        let config = ProcessConfig::default()
            .with_io(ProcessIo::captured(CapturedIo::new()))
            .with_timeout(Some(self.timeout))
            .with_max_output_bytes(self.max_output_bytes);

        let result = run(&spec, &config)?;
        if result.timed_out {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Timeout,
                format!(
                    "toolchain probe '{}' timed out after {:?} in '{}'",
                    probe.label,
                    self.timeout,
                    workspace_root.display()
                ),
            ));
        }
        if result.stdout_truncated {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "toolchain probe '{}' output in '{}' exceeded {} bytes",
                    probe.label,
                    workspace_root.display(),
                    self.max_output_bytes
                ),
            ));
        }
        if !result.success() {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "toolchain probe '{}' failed in '{}' (exit {:?}): {}",
                    probe.label,
                    workspace_root.display(),
                    result.exit_code,
                    result.stderr.trim()
                ),
            ));
        }
        let version = result.stdout.trim();
        if version.is_empty() {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "toolchain probe '{}' in '{}' returned an empty version",
                    probe.label,
                    workspace_root.display()
                ),
            ));
        }
        Ok(version.to_string())
    }
}

/// Resolve the toolchain identity for every active workspace.
///
/// Returns a map from workspace id to its version-stamped [`ToolchainTag`]; only
/// workspaces owning an active module are probed.
///
/// # Errors
/// An active module referencing an unknown workspace or ecosystem adapter, or a
/// failing probe.
pub(super) fn resolve(
    project_root: &AbsPath,
    federation: &Federation,
    active: &BTreeSet<ModuleKey>,
    adapters: &MemberAdapters,
    prober: &dyn ToolchainProber,
) -> AppResult<BTreeMap<WorkspaceId, ToolchainTag>> {
    let active_workspaces = active_workspaces(federation, active)?;

    let mut resolved = BTreeMap::new();
    for (workspace_id, (ecosystem, member)) in active_workspaces {
        let workspace = find_workspace(&workspace_id, federation)?;
        let adapter = adapters.get(member.as_ref(), &ecosystem).ok_or_else(|| {
            AppError::new(rskit_errors::ErrorCode::Internal, format!(
                "no configured adapter for ecosystem '{ecosystem}' owning workspace '{workspace_id}'"
            ))
        })?;
        let probe = adapter.toolchain_probe();
        let root =
            safe_join(project_root.as_path(), workspace.root.as_path()).map_err(|error| {
                AppError::invalid_input("workspace.root", error.to_string()).with_cause(error)
            })?;
        let version = prober.probe(&probe, &root)?;
        resolved.insert(
            workspace_id,
            workspace.toolchain.clone().with_version(version),
        );
    }
    Ok(resolved)
}

/// Map each active workspace id to the ecosystem and member owning it.
///
/// # Errors
/// Two active modules with different ecosystem or member owners claiming the
/// same workspace id is an internal inconsistency (it would pick one adapter's
/// probe arbitrarily).
fn active_workspaces(
    federation: &Federation,
    active: &BTreeSet<ModuleKey>,
) -> AppResult<BTreeMap<WorkspaceId, (EcosystemId, Option<MemberId>)>> {
    let mut workspaces: BTreeMap<WorkspaceId, (EcosystemId, Option<MemberId>)> = BTreeMap::new();
    for module in &federation.modules {
        if !active.contains(&module.key()) {
            continue;
        }
        if let Some(workspace) = &module.workspace {
            match workspaces.get(workspace) {
                Some((existing_ecosystem, existing_member))
                    if existing_ecosystem != &module.id.ecosystem
                        || existing_member != &module.member =>
                {
                    return Err(AppError::new(
                        rskit_errors::ErrorCode::Internal,
                        format!(
                            "workspace '{workspace}' is claimed by '{}' and '{}'",
                            workspace_owner(existing_ecosystem, existing_member.as_ref()),
                            workspace_owner(&module.id.ecosystem, module.member.as_ref())
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    workspaces.insert(
                        workspace.clone(),
                        (module.id.ecosystem.clone(), module.member.clone()),
                    );
                }
            }
        }
    }
    Ok(workspaces)
}

fn workspace_owner(ecosystem: &EcosystemId, member: Option<&MemberId>) -> String {
    member.map_or_else(
        || ecosystem.to_string(),
        |member| format!("{member}/{ecosystem}"),
    )
}

/// Look up a workspace by id in the federation.
fn find_workspace<'a>(
    workspace_id: &WorkspaceId,
    federation: &'a Federation,
) -> AppResult<&'a Workspace> {
    federation
        .workspaces
        .iter()
        .find(|workspace| &workspace.id == workspace_id)
        .ok_or_else(|| {
            AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!("active module references unknown workspace '{workspace_id}'"),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use toven_model::{EcosystemId, MemberId, Module, ModuleRef, RepoPath, WorkspaceId};

    use super::active_workspaces;
    use crate::plan::discover::Federation;

    fn module(member: &str, name: &str, workspace: &str) -> Module {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            RepoPath::new(format!("repos/{member}/{name}")).unwrap(),
        );
        module.member = Some(MemberId::new(member).unwrap());
        module.workspace = Some(WorkspaceId::new(workspace).unwrap());
        module
    }

    #[test]
    fn active_workspaces_rejects_same_workspace_claimed_by_different_members() {
        let core = module("core", "lib", "rust");
        let services = module("services", "api", "rust");
        let active = BTreeSet::from([core.key(), services.key()]);
        let federation = Federation {
            workspaces: Vec::new(),
            modules: vec![core, services],
            edges: Vec::new(),
            warnings: Vec::new(),
        };

        let error = active_workspaces(&federation, &active)
            .expect_err("workspace ownership must include the member");

        assert!(
            error.to_string().contains("core/rust") && error.to_string().contains("services/rust"),
            "error should identify both workspace owners: {error}"
        );
    }
}
