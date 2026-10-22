//! Named task sets (`[sets.<name>] tasks = [...]`) and the task-list token.
//!
//! A run may name several tasks in one comma-separated token
//! (`toven run build,test,lint`), and a set names such a list once in config
//! (`toven verify`). The tasks are independent: they are planned into one plan
//! and run side by side, so list order is not run order.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::validate_path_safe_identifier;
use serde::{Deserialize, Serialize};
use toven_ports::TaskKind;

/// The separator between task names in one task-list token.
pub const TASK_LIST_SEPARATOR: char = ',';

/// One `[sets.<name>]` entry: the tasks the set runs together.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSetConfig {
    /// The task names the set plans into one run. Must be non-empty and name
    /// tasks, not other sets.
    pub tasks: Vec<String>,
}

/// Validate every `[sets.<name>]`: safe names that are not built-in task
/// kinds, and non-empty flat task lists.
pub(super) fn validate_sets(sets: &BTreeMap<String, TaskSetConfig>) -> AppResult<()> {
    for (name, set) in sets {
        let field = format!("sets.{name}");
        validate_path_safe_identifier(&field, name)?;
        reject_separator(&field, name)?;
        if TaskKind::from_name(name).is_some() {
            return Err(AppError::invalid_input(
                field,
                format!("'{name}' is a built-in task kind; name the set something else"),
            ));
        }
        if set.tasks.is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.tasks"),
                "a set must name at least one task",
            ));
        }
        let mut seen = BTreeSet::new();
        for task in &set.tasks {
            let task_field = format!("{field}.tasks");
            if task.trim().is_empty() {
                return Err(AppError::invalid_input(
                    task_field,
                    "task names cannot be empty",
                ));
            }
            reject_separator(&task_field, task)?;
            if sets.contains_key(task) {
                return Err(AppError::invalid_input(
                    task_field,
                    format!("'{task}' is a set; a set lists tasks, not other sets"),
                ));
            }
            if !seen.insert(task.as_str()) {
                return Err(AppError::invalid_input(
                    task_field,
                    format!("'{task}' is listed twice"),
                ));
            }
        }
    }
    Ok(())
}

fn reject_separator(field: &str, name: &str) -> AppResult<()> {
    if name.contains(TASK_LIST_SEPARATOR) {
        return Err(AppError::invalid_input(
            field,
            format!("cannot contain the task-list separator '{TASK_LIST_SEPARATOR}'"),
        ));
    }
    Ok(())
}

/// Expand a task-list token (`build,test`, a set name, or a mix) into its
/// distinct task names, first occurrence first.
///
/// # Errors
/// Returns [`AppError::invalid_input`] for an empty entry (`build,,test`).
pub(super) fn expand_task_list(
    token: &str,
    sets: &BTreeMap<String, TaskSetConfig>,
) -> AppResult<Vec<String>> {
    let mut tasks = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in token.split(TASK_LIST_SEPARATOR) {
        if entry.is_empty() {
            return Err(AppError::invalid_input(
                "task",
                format!(
                    "'{token}' has an empty entry; separate task names with one '{TASK_LIST_SEPARATOR}'"
                ),
            ));
        }
        let expanded = sets
            .get(entry)
            .map_or_else(|| vec![entry.to_string()], |set| set.tasks.clone());
        for task in expanded {
            if seen.insert(task.clone()) {
                tasks.push(task);
            }
        }
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{TaskSetConfig, expand_task_list, validate_sets};

    fn sets(entries: &[(&str, &[&str])]) -> BTreeMap<String, TaskSetConfig> {
        entries
            .iter()
            .map(|(name, tasks)| {
                (
                    (*name).to_string(),
                    TaskSetConfig {
                        tasks: tasks.iter().map(|task| (*task).to_string()).collect(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_plain_task_is_a_one_item_list() {
        assert_eq!(expand_task_list("test", &sets(&[])).unwrap(), ["test"]);
    }

    #[test]
    fn a_comma_list_mixes_tasks_and_sets_without_duplicates() {
        let sets = sets(&[("verify", &["lint", "test"])]);
        assert_eq!(
            expand_task_list("build,verify,test", &sets).unwrap(),
            ["build", "lint", "test"]
        );
    }

    #[test]
    fn an_empty_entry_is_rejected() {
        assert!(expand_task_list("build,,test", &sets(&[])).is_err());
        assert!(expand_task_list("build,", &sets(&[])).is_err());
    }

    #[test]
    fn valid_sets_pass() {
        validate_sets(&sets(&[("verify", &["lint", "test"])])).unwrap();
    }

    #[test]
    fn malformed_sets_are_rejected() {
        assert!(validate_sets(&sets(&[("verify", &[])])).is_err());
        assert!(validate_sets(&sets(&[("a,b", &["lint"])])).is_err());
        assert!(validate_sets(&sets(&[("verify", &["lint,test"])])).is_err());
        assert!(validate_sets(&sets(&[("verify", &["lint", "lint"])])).is_err());
        assert!(validate_sets(&sets(&[("all", &["verify"]), ("verify", &["lint"])])).is_err());
        assert!(validate_sets(&sets(&[("verify", &[" "])])).is_err());
        assert!(validate_sets(&sets(&[("test", &["lint"])])).is_err());
    }
}
