//! Group-scoped task/strategy overrides resolved per active module.
//!
//! A `[groups.<name>]` may carry a `run_strategy` and a `tasks` map that layer
//! on top of the ecosystem/adapter defaults for *its members only*. The Graph
//! phase already resolves each group's membership to concrete [`ModuleKey`]s;
//! this module folds those resolved memberships into a per-module lookup the
//! [`schedule`](super::schedule) phase consults when it selects each module's
//! effective task and wave-ordering strategy.
//!
//! Precedence is deterministic and fails closed: a module reached by two
//! different groups that both override the same task (or both set
//! `run_strategy`) is a hard error rather than an implicit last-writer-wins.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::ModuleKey;
use toven_ports::{RunStrategy, TaskOverride};

use crate::config::GroupConfig;

/// The resolved group overrides for every module a group touches.
///
/// Empty when no group declares a `tasks`/`run_strategy` override, in which
/// case scheduling falls back entirely to the ecosystem/adapter defaults.
#[derive(Debug, Clone, Default)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct GroupOverrides {
    per_module: BTreeMap<ModuleKey, ModuleOverride>,
}

/// The overrides a single module inherits from the group(s) it belongs to.
#[derive(Debug, Clone, Default)]
struct ModuleOverride {
    /// Task-name → (declaring identity, override). At most one declaration per
    /// task name; the identity is the scope-qualified, id-safe group identity.
    tasks: BTreeMap<String, (String, TaskOverride)>,
    /// The declaring identity and its wave-ordering override, if any.
    run_strategy: Option<(String, RunStrategy)>,
}

impl GroupOverrides {
    /// Fold one group's `tasks`/`run_strategy` overrides onto each of its
    /// resolved members.
    ///
    /// `identity` is the scope-qualified, id-safe group identity (see
    /// `override_identity` in [`graph`](super::graph)). It is used both to
    /// detect conflicts between distinct declarations that may share a plain
    /// name (a member-local group and an umbrella group both called
    /// `integration`) and as the token folded into batch unit ids so members
    /// carrying overrides from distinct declarations never collapse into one
    /// argv.
    ///
    /// # Errors
    /// A member already carrying a `run_strategy` or same-named task override
    /// from a *different* declaration — overlapping groups must not disagree.
    pub(crate) fn record(
        &mut self,
        identity: &str,
        group: &GroupConfig,
        members: &BTreeSet<ModuleKey>,
    ) -> AppResult<()> {
        if group.run_strategy.is_none() && group.tasks.is_empty() {
            return Ok(());
        }
        for key in members {
            let entry = self.per_module.entry(key.clone()).or_default();
            if let Some(strategy) = group.run_strategy {
                match &entry.run_strategy {
                    Some((prior, _)) if prior != identity => {
                        return Err(conflict(key, "run_strategy", prior, identity));
                    }
                    _ => entry.run_strategy = Some((identity.to_string(), strategy)),
                }
            }
            for (task, over) in &group.tasks {
                match entry.tasks.get(task) {
                    Some((prior, _)) if prior != identity => {
                        return Err(conflict(key, &format!("task '{task}'"), prior, identity));
                    }
                    _ => {
                        entry
                            .tasks
                            .insert(task.clone(), (identity.to_string(), over.clone()));
                    }
                }
            }
        }
        Ok(())
    }

    /// The group `run_strategy` override for `module`, if any group set one.
    pub(crate) fn run_strategy(&self, module: &ModuleKey) -> Option<RunStrategy> {
        self.per_module
            .get(module)
            .and_then(|over| over.run_strategy.as_ref())
            .map(|(_, strategy)| *strategy)
    }

    /// The group task override named `task` for `module`, paired with the
    /// declaring group identity (folded into the batch unit id to keep members
    /// overridden by distinct declarations in separate units).
    pub(crate) fn task(&self, module: &ModuleKey, task: &str) -> Option<(&str, &TaskOverride)> {
        self.per_module
            .get(module)
            .and_then(|over| over.tasks.get(task))
            .map(|(identity, over)| (identity.as_str(), over))
    }
}

/// A typed conflict: two declarations disagree on the same module's override
/// slot.
fn conflict(module: &ModuleKey, slot: &str, prior: &str, next: &str) -> AppError {
    AppError::invalid_input(
        "groups",
        format!(
            "module '{module}' {slot} is overridden by conflicting groups ({prior}) and ({next})"
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::{RunStrategy, TaskOverride};

    use super::GroupOverrides;
    use crate::config::GroupConfig;

    fn key(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap())
    }

    fn members(names: &[&str]) -> BTreeSet<ModuleKey> {
        names.iter().map(|name| key(name)).collect()
    }

    fn test_override() -> TaskOverride {
        TaskOverride {
            argv: Some(vec!["cargo".into(), "nextest".into(), "run".into()]),
            ..TaskOverride::default()
        }
    }

    fn task_group() -> GroupConfig {
        GroupConfig {
            tasks: std::collections::BTreeMap::from([("test".to_string(), test_override())]),
            ..GroupConfig::default()
        }
    }

    fn strategy_group() -> GroupConfig {
        GroupConfig {
            run_strategy: Some(RunStrategy::Unordered),
            ..GroupConfig::default()
        }
    }

    #[test]
    fn records_run_strategy_and_task_for_members() {
        let mut overrides = GroupOverrides::default();
        let group = GroupConfig {
            run_strategy: Some(RunStrategy::Unordered),
            tasks: std::collections::BTreeMap::from([("test".to_string(), test_override())]),
            ..GroupConfig::default()
        };

        overrides
            .record("integration", &group, &members(&["it"]))
            .expect("records");

        assert_eq!(
            overrides.run_strategy(&key("it")),
            Some(RunStrategy::Unordered)
        );
        let (identity, over) = overrides.task(&key("it"), "test").expect("task override");
        assert_eq!(identity, "integration");
        assert_eq!(over.argv.as_deref().unwrap(), ["cargo", "nextest", "run"]);
        // A non-member sees nothing.
        assert!(overrides.run_strategy(&key("other")).is_none());
        assert!(overrides.task(&key("other"), "test").is_none());
    }

    #[test]
    fn empty_group_records_nothing() {
        let mut overrides = GroupOverrides::default();
        overrides
            .record("plain", &GroupConfig::default(), &members(&["it"]))
            .expect("records");
        assert!(overrides.run_strategy(&key("it")).is_none());
    }

    #[test]
    fn conflicting_task_override_across_groups_is_rejected() {
        let mut overrides = GroupOverrides::default();
        let group = task_group();

        overrides
            .record("first", &group, &members(&["it"]))
            .expect("first records");
        let error = overrides
            .record("second", &group, &members(&["it"]))
            .expect_err("conflict rejected");
        assert!(error.to_string().contains("conflicting groups"), "{error}");
    }

    #[test]
    fn conflicting_run_strategy_across_groups_is_rejected() {
        let mut overrides = GroupOverrides::default();
        let group = strategy_group();

        overrides
            .record("first", &group, &members(&["it"]))
            .expect("first records");
        let error = overrides
            .record("second", &group, &members(&["it"]))
            .expect_err("conflict rejected");
        assert!(error.to_string().contains("run_strategy"), "{error}");
    }

    #[test]
    fn same_name_groups_in_different_scopes_have_distinct_identities() {
        // A member-local group and an umbrella group that share the plain name
        // `integration` are distinct declarations: overlapping them on one module must
        // fail closed, and their fold identities must differ so members overridden by
        // each never collapse into one batch unit.
        let mut overrides = GroupOverrides::default();
        let group = task_group();

        overrides
            .record("member.billing.integration", &group, &members(&["it"]))
            .expect("member-local records");
        let error = overrides
            .record("umbrella.integration", &group, &members(&["it"]))
            .expect_err("cross-scope conflict rejected");
        assert!(error.to_string().contains("conflicting groups"), "{error}");

        // The surviving member-local identity is the one folded into unit ids.
        let (identity, _) = overrides.task(&key("it"), "test").expect("task override");
        assert_eq!(identity, "member.billing.integration");
    }
}
