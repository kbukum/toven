//! The Go wizard `render` step: author the complete `[ecosystems.go]` section.
//!
//! The config is the authoritative task source: `toven init` writes the whole
//! `go` command table into `toven.toml` at onboarding time, so the planner reads
//! runnable tasks straight from config rather than a compiled-in adapter default.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use toml::Table;
use toven_ports::{Answers, Detection, EcosystemFragment, FanOut, Readiness, TaskEntry};

use crate::detect::{GoFacts, ROOT_MANIFEST};

/// The workspace-level cache inputs every `go` task shares.
const SHARED_INPUTS: [&str; 3] = ["go.sum", "go.work", "go.work.sum"];

/// The package pattern a per-module `go` invocation targets.
const PACKAGE_PATTERN: &str = "./...";

/// The serializable `[ecosystems.go]` body: discovered modules plus task table.
#[derive(Debug, Serialize)]
struct GoFragmentBody {
    modules: Vec<String>,
    tasks: BTreeMap<String, TaskEntry>,
}

/// Render the complete `[ecosystems.go]` fragment from a [`Detection`].
///
/// Go currently has no questionnaire choices, so `answers` is accepted for the
/// uniform provider contract but does not affect the canonical task table.
///
/// # Errors
/// Propagates a facts-decode failure or a TOML encoding failure.
pub(crate) fn render(detection: &Detection, _answers: &Answers) -> AppResult<EcosystemFragment> {
    GoFacts::from_detection(detection)?;

    let body = GoFragmentBody {
        modules: vec![ROOT_MANIFEST.to_string()],
        tasks: task_table(),
    };
    let table = Table::try_from(&body).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to encode go fragment").with_cause(error)
    })?;

    Ok(EcosystemFragment::new(detection.ecosystem.clone(), table))
}

/// Build the complete canonical Go task table.
fn task_table() -> BTreeMap<String, TaskEntry> {
    let mut tasks = BTreeMap::new();
    tasks.insert("build".to_string(), module_entry("build"));
    tasks.insert("check".to_string(), module_entry("vet"));
    tasks.insert("format".to_string(), module_entry("fmt"));
    tasks.insert("lint".to_string(), module_entry("vet"));
    tasks.insert("test".to_string(), module_entry("test"));
    tasks.insert("doc".to_string(), doc_entry());
    tasks.insert("run".to_string(), run_entry());
    tasks
}

/// The workspace-level shared cache inputs as an owned vector.
fn shared_inputs() -> Vec<String> {
    SHARED_INPUTS
        .iter()
        .map(|input| (*input).to_string())
        .collect()
}

/// A `go -C {module.root} <subcommand>` entry fanning out per module.
fn module_entry(subcommand: &str) -> TaskEntry {
    TaskEntry {
        kind: None,
        argv: vec![
            "go".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            subcommand.to_string(),
            "{args}".to_string(),
            "{module.selector}".to_string(),
        ],
        selector: vec![PACKAGE_PATTERN.to_string()],
        fan_out: FanOut::PerModule,
        persistent: false,
        readiness: Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        shared_inputs: shared_inputs(),
    }
}

/// The `go doc` entry, which targets the module root without `./...`.
fn doc_entry() -> TaskEntry {
    TaskEntry {
        kind: None,
        argv: vec![
            "go".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            "doc".to_string(),
            "{args}".to_string(),
        ],
        selector: Vec::new(),
        fan_out: FanOut::PerModule,
        persistent: false,
        readiness: Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        shared_inputs: shared_inputs(),
    }
}

/// The persistent `go run .` entry.
fn run_entry() -> TaskEntry {
    TaskEntry {
        kind: None,
        argv: vec![
            "go".to_string(),
            "-C".to_string(),
            "{module.root}".to_string(),
            "run".to_string(),
            ".".to_string(),
            "{args}".to_string(),
        ],
        selector: Vec::new(),
        fan_out: FanOut::PerModule,
        persistent: true,
        readiness: Readiness::Started,
        readiness_timeout_secs: None,
        cache_args: false,
        shared_inputs: shared_inputs(),
    }
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_ports::{Answers, Detection, FanOut};

    use super::render;
    use crate::config::GoConfig;
    use crate::detect::GoFacts;

    fn detection() -> Detection {
        let facts = GoFacts {
            manifest: "go.mod".to_string(),
        };
        Detection::new(
            toven_model::EcosystemId::new("go").unwrap(),
            Table::try_from(&facts).unwrap(),
        )
    }

    fn parse(fragment: &toven_ports::EcosystemFragment) -> GoConfig {
        fragment
            .table
            .clone()
            .try_into()
            .expect("fragment parses back through GoConfig")
    }

    #[test]
    fn authors_the_complete_task_table() {
        let fragment = render(&detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let mut names: Vec<&str> = config.common.tasks.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["build", "check", "doc", "format", "lint", "run", "test"]
        );
        assert_eq!(config.modules, ["go.mod"]);
    }

    #[test]
    fn test_task_uses_go_package_selector_and_workspace_inputs() {
        let fragment = render(&detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let test = config.common.tasks.get("test").expect("test task");
        assert_eq!(
            test.argv,
            [
                "go",
                "-C",
                "{module.root}",
                "test",
                "{args}",
                "{module.selector}"
            ]
        );
        assert_eq!(test.selector, ["./..."]);
        assert_eq!(test.shared_inputs, ["go.sum", "go.work", "go.work.sum"]);
        assert_eq!(test.fan_out, FanOut::PerModule);
    }

    #[test]
    fn run_task_is_persistent_and_doc_has_no_selector() {
        let fragment = render(&detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        let run = config.common.tasks.get("run").expect("run task");
        assert!(run.persistent);
        assert_eq!(
            run.argv,
            ["go", "-C", "{module.root}", "run", ".", "{args}"]
        );

        let doc = config.common.tasks.get("doc").expect("doc task");
        assert!(doc.selector.is_empty());
        assert_eq!(doc.argv, ["go", "-C", "{module.root}", "doc", "{args}"]);
    }

    #[test]
    fn rendered_table_round_trips_through_materialize() {
        let fragment = render(&detection(), &Answers::new()).expect("render");
        let config = parse(&fragment);
        for (key, entry) in &config.common.tasks {
            entry
                .materialize("go", key)
                .expect("every authored entry materializes");
        }
    }
}
