//! CLI-sourced module selection: the bridge from argv selection flags to the
//! engine [`Selection`].
//!
//! Both the execution verbs ([`run`](super::run)) and `affected`
//! ([`introspect`](super::introspect)) share one resolution so the same argv
//! (`--base`/`--merge-base` changed-selection vs. `--module`/`--workspace`
//! explicit selection) maps to the same engine intent everywhere.

use rskit_errors::{AppError, AppResult};
use toven_engine::plan::{ModuleSelector, Selection};
use toven_engine::vcs::{BaselineFlags, BaselineStrategy};
use toven_model::{ModuleRef, WorkspaceId};

/// The CLI-sourced inputs that determine the engine [`Selection`].
///
/// Bundles the changed-selection baseline (`--base`/`--merge-base`) with the
/// explicit graph targets (`--module`/`--workspace`, `--with-dependents`) so the
/// selection verbs thread one value instead of a widening argument list.
#[derive(Debug, Default, Clone)]
pub(crate) struct TaskSelection {
    /// The changed-selection baseline flags.
    pub(crate) baseline: BaselineFlags,
    /// `--module <ref>` targets (`ecosystem:name`), repeatable.
    pub(crate) modules: Vec<String>,
    /// `--workspace <id>` targets, repeatable.
    pub(crate) workspaces: Vec<String>,
    /// `--with-dependents`: also activate the reverse-dependents closure.
    pub(crate) with_dependents: bool,
}

impl TaskSelection {
    /// Resolve the CLI selection flags into the engine [`Selection`].
    ///
    /// Explicit targets (`--module`/`--workspace`) take precedence and produce
    /// [`Selection::Explicit`]; they are mutually exclusive with the changed
    /// baseline (`--base`/`--merge-base`). With no explicit target the result is
    /// [`Selection::All`] (no baseline) or [`Selection::Changed`] (baseline set),
    /// where `base_ref` is the project's configured fallback baseline
    /// (`[project].base_ref`).
    ///
    /// # Errors
    /// Returns a typed usage error when explicit targets are combined with a
    /// baseline, when `--with-dependents` is used without an explicit target, or
    /// when a `--module`/`--workspace` value fails to parse.
    pub(crate) fn resolve(&self, base_ref: Option<&str>) -> AppResult<Selection> {
        let baseline = &self.baseline;
        let has_explicit = !self.modules.is_empty() || !self.workspaces.is_empty();
        let has_baseline = baseline.base.is_some() || baseline.merge_base;

        if has_explicit {
            if has_baseline {
                return Err(AppError::invalid_input(
                    "flags",
                    "`--module`/`--workspace` select the graph explicitly and cannot be combined with `--base`/`--merge-base` (changed selection)",
                ));
            }
            let mut targets = Vec::with_capacity(self.modules.len() + self.workspaces.len());
            for reference in &self.modules {
                targets.push(ModuleSelector::Module(ModuleRef::parse(reference)?));
            }
            for workspace in &self.workspaces {
                targets.push(ModuleSelector::Workspace(WorkspaceId::new(workspace)?));
            }
            return Ok(Selection::Explicit {
                targets,
                include_dependents: self.with_dependents,
            });
        }

        if self.with_dependents {
            return Err(AppError::invalid_input(
                "flags",
                "`--with-dependents` only applies together with `--module`/`--workspace`",
            ));
        }

        if !has_baseline {
            return Ok(Selection::All);
        }
        Ok(Selection::Changed(BaselineStrategy::resolve_optional(
            baseline, base_ref,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::TaskSelection;
    use toven_engine::plan::{ModuleSelector, Selection};
    use toven_engine::vcs::BaselineFlags;
    use toven_model::{ModuleRef, WorkspaceId};

    fn selection() -> TaskSelection {
        TaskSelection::default()
    }

    #[test]
    fn no_flags_resolves_to_full_selection() {
        assert_eq!(selection().resolve(None).unwrap(), Selection::All);
    }

    #[test]
    fn a_baseline_alone_resolves_to_changed_selection() {
        let resolved = TaskSelection {
            baseline: BaselineFlags::new().with_merge_base(true),
            ..selection()
        }
        .resolve(Some("origin/main"))
        .unwrap();

        assert!(matches!(resolved, Selection::Changed(_)));
    }

    #[test]
    fn explicit_targets_resolve_to_explicit_selection() {
        let resolved = TaskSelection {
            modules: vec!["rust:core".into()],
            workspaces: vec!["rust".into()],
            with_dependents: true,
            ..selection()
        }
        .resolve(None)
        .unwrap();

        assert_eq!(
            resolved,
            Selection::Explicit {
                targets: vec![
                    ModuleSelector::Module(ModuleRef::parse("rust:core").unwrap()),
                    ModuleSelector::Workspace(WorkspaceId::new("rust").unwrap()),
                ],
                include_dependents: true,
            }
        );
    }

    #[test]
    fn explicit_targets_combined_with_a_baseline_is_a_usage_error() {
        let error = TaskSelection {
            baseline: BaselineFlags::new().with_merge_base(true),
            modules: vec!["rust:core".into()],
            ..selection()
        }
        .resolve(None)
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("--base"), "{message}");
        assert!(message.contains("--module"), "{message}");
    }

    #[test]
    fn with_dependents_without_an_explicit_target_is_a_usage_error() {
        let error = TaskSelection {
            with_dependents: true,
            ..selection()
        }
        .resolve(None)
        .unwrap_err();

        assert!(error.to_string().contains("--with-dependents"), "{error}");
    }

    #[test]
    fn a_malformed_module_ref_is_a_typed_error() {
        let error = TaskSelection {
            modules: vec!["not a ref".into()],
            ..selection()
        }
        .resolve(None)
        .unwrap_err();

        assert!(!error.to_string().is_empty());
    }
}
