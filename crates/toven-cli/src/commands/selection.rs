//! CLI-sourced module selection: the bridge from argv selection flags to the
//! engine [`Selection`].
//!
//! Both the execution verbs ([`run`](super::run)) and `affected`
//! ([`introspect`](super::introspect)) share one resolution so the same argv
//! (`--base`/`--merge-base` changed-selection vs. `--module`/`--workspace`
//! explicit selection) maps to the same engine intent everywhere.

use rskit_errors::{AppError, AppResult};
use toven_engine_core::plan::{ModuleSelector, Selection};
use toven_engine_core::vcs::{BaselineFlags, BaselineStrategy};

/// The CLI-sourced inputs that determine the engine [`Selection`].
///
/// Bundles the changed-selection baseline (`--base`/`--merge-base`) with the
/// explicit graph targets (`--module`/`--workspace` plus the `--dependencies`/
/// `--dependents` closures) so the selection verbs thread one value instead of
/// a widening argument list.
#[derive(Debug, Default, Clone)]
pub(crate) struct TaskSelection {
    /// The changed-selection baseline flags.
    pub(crate) baseline: BaselineFlags,
    /// `--module <sel>` targets (the selector grammar), repeatable.
    pub(crate) modules: Vec<String>,
    /// `--workspace <sel>` targets, repeatable.
    pub(crate) workspaces: Vec<String>,
    /// `--dependents`: also activate the reverse-dependents closure.
    pub(crate) with_dependents: bool,
    /// `--dependencies`: also activate the forward-dependencies closure.
    pub(crate) with_dependencies: bool,
}

impl TaskSelection {
    /// Resolve the CLI selection flags into the engine [`Selection`].
    ///
    /// Explicit targets (`--module`/`--workspace`) take precedence and produce
    /// [`Selection::Explicit`]; they are mutually exclusive with the changed
    /// baseline (`--base`/`--merge-base`). With no explicit target the result
    /// is [`Selection::All`] (no baseline) or [`Selection::Changed`] (baseline
    /// set), where `base_ref` is the project's configured fallback baseline
    /// (`[project].base_ref`).
    ///
    /// # Errors
    /// Returns a typed usage error when explicit targets are combined with a
    /// baseline, when `--dependencies`/`--dependents` is used without an
    /// explicit target, or when a `--module`/`--workspace` value fails to
    /// parse.
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
                targets.push(ModuleSelector::parse(reference)?);
            }
            for workspace in &self.workspaces {
                targets.push(ModuleSelector::whole_workspace(workspace)?);
            }
            return Ok(Selection::Explicit {
                targets,
                include_dependents: self.with_dependents,
                include_dependencies: self.with_dependencies,
            });
        }

        if self.with_dependents || self.with_dependencies {
            return Err(AppError::invalid_input(
                "flags",
                "`--dependencies`/`--dependents` only apply together with `--module`/`--workspace`",
            ));
        }

        if !has_baseline {
            return Ok(Selection::All);
        }
        Ok(Selection::Changed(BaselineStrategy::resolve_optional(
            baseline, base_ref,
        )))
    }

    /// Split the CLI selection into an `explain` plan scope and an optional
    /// display focus.
    ///
    /// An explicit `--module`/`--workspace` selection becomes the display
    /// *focus* (which units to show) while the plan is built over the full set
    /// ([`Selection::All`]), so each shown unit is the real batched unit the
    /// target belongs to rather than a synthetic single-module cut. A
    /// changed/no-baseline selection has no focus: the plan is built over that
    /// selection and every scheduled unit is shown.
    ///
    /// # Errors
    /// Propagates the same usage/parse errors as [`resolve`](Self::resolve).
    pub(crate) fn resolve_explain(
        &self,
        base_ref: Option<&str>,
    ) -> AppResult<(Selection, Option<Selection>)> {
        let selection = self.resolve(base_ref)?;
        Ok(match selection {
            explicit @ Selection::Explicit { .. } => (Selection::All, Some(explicit)),
            scope => (scope, None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::TaskSelection;
    use toven_engine_core::plan::{ModuleSelector, Selection};
    use toven_engine_core::vcs::BaselineFlags;

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
            with_dependencies: true,
            ..selection()
        }
        .resolve(None)
        .unwrap();

        assert_eq!(
            resolved,
            Selection::Explicit {
                targets: vec![
                    ModuleSelector::parse("rust:core").unwrap(),
                    ModuleSelector::whole_workspace("rust").unwrap(),
                ],
                include_dependents: true,
                include_dependencies: true,
            }
        );
    }

    #[test]
    fn a_bare_name_selector_is_accepted() {
        let resolved = TaskSelection {
            modules: vec!["core".into()],
            ..selection()
        }
        .resolve(None)
        .unwrap();

        assert_eq!(
            resolved,
            Selection::Explicit {
                targets: vec![ModuleSelector::parse("core").unwrap()],
                include_dependents: false,
                include_dependencies: false,
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
    fn dependents_without_an_explicit_target_is_a_usage_error() {
        let error = TaskSelection {
            with_dependents: true,
            ..selection()
        }
        .resolve(None)
        .unwrap_err();

        assert!(error.to_string().contains("--dependents"), "{error}");
    }

    #[test]
    fn dependencies_without_an_explicit_target_is_a_usage_error() {
        let error = TaskSelection {
            with_dependencies: true,
            ..selection()
        }
        .resolve(None)
        .unwrap_err();

        assert!(error.to_string().contains("--dependencies"), "{error}");
    }

    #[test]
    fn a_malformed_module_selector_is_a_typed_error() {
        let error = TaskSelection {
            modules: vec![":core".into()],
            ..selection()
        }
        .resolve(None)
        .unwrap_err();

        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn explain_split_promotes_an_explicit_selection_to_a_focus_over_the_full_plan() {
        let (scope, focus) = TaskSelection {
            modules: vec!["rust:core".into()],
            with_dependents: true,
            ..selection()
        }
        .resolve_explain(None)
        .unwrap();

        assert_eq!(scope, Selection::All);
        assert_eq!(
            focus,
            Some(Selection::Explicit {
                targets: vec![ModuleSelector::parse("rust:core").unwrap()],
                include_dependents: true,
                include_dependencies: false,
            })
        );
    }

    #[test]
    fn explain_split_has_no_focus_without_an_explicit_selection() {
        assert_eq!(
            selection().resolve_explain(None).unwrap(),
            (Selection::All, None)
        );

        let (scope, focus) = TaskSelection {
            baseline: BaselineFlags::new().with_merge_base(true),
            ..selection()
        }
        .resolve_explain(Some("origin/main"))
        .unwrap();
        assert!(matches!(scope, Selection::Changed(_)));
        assert_eq!(focus, None);
    }
}
