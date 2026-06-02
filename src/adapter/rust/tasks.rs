//! Rust adapter task templates.

use std::{collections::BTreeMap, path::Path};

use crate::{
    adapter::defaults::argv_task,
    core::{AdapterId, AppResult, Task, validate_shared_inputs},
    generate::GeneratedTask,
};

const DEFAULT_TASKS: &[RustTaskTemplate] = &[
    RustTaskTemplate::new("bench", "bench"),
    RustTaskTemplate::new("build", "build"),
    RustTaskTemplate::new("check", "check"),
    RustTaskTemplate::new("clippy", "clippy"),
    RustTaskTemplate::new("doc", "doc"),
    RustTaskTemplate::new("fmt", "fmt"),
    RustTaskTemplate::new("fmt-check", "fmt").with_extra_args(&["--check"]),
    RustTaskTemplate::new("test", "test"),
];

const BROAD_SHARED_INPUT_CANDIDATES: &[&str] = &[
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    ".cargo/config.toml",
    ".cargo/config",
];

const CLIPPY_SHARED_INPUT_CANDIDATES: &[&str] = &["clippy.toml", ".clippy.toml"];
const RUSTFMT_SHARED_INPUT_CANDIDATES: &[&str] = &["rustfmt.toml", ".rustfmt.toml"];

struct RustTaskTemplate {
    name: &'static str,
    cargo_command: &'static str,
    extra_args: &'static [&'static str],
}

/// Runtime fallback tasks for minimal hand-written Rust configs.
pub(super) fn default_tasks(adapter_id: &AdapterId) -> Vec<Task> {
    DEFAULT_TASKS
        .iter()
        .map(|template| {
            argv_task(
                adapter_id.clone(),
                template.name,
                template.argv(PackageSelector::Inline),
            )
        })
        .collect()
}

/// Generated task definitions for reviewable committed configs.
pub(super) fn generated_tasks(root: &Path) -> AppResult<BTreeMap<String, GeneratedTask>> {
    let shared_inputs = existing_shared_inputs(root);
    validate_shared_inputs(
        "profiles.<profile>.tasks.<task>.shared_inputs",
        &shared_inputs,
    )?;

    Ok(DEFAULT_TASKS
        .iter()
        .map(|template| {
            (
                template.name.to_string(),
                GeneratedTask {
                    argv: template.argv(PackageSelector::ModuleArgs),
                    cache_args: false,
                    shared_inputs: template.shared_inputs(root, &shared_inputs),
                    persistent: false,
                    ready_on: None,
                    ready_command: None,
                    ready_output: None,
                    ready_timeout_seconds: None,
                },
            )
        })
        .collect())
}

impl RustTaskTemplate {
    const fn new(name: &'static str, cargo_command: &'static str) -> Self {
        Self {
            name,
            cargo_command,
            extra_args: &[],
        }
    }

    const fn with_extra_args(mut self, extra_args: &'static [&'static str]) -> Self {
        self.extra_args = extra_args;
        self
    }

    fn argv(&self, selector: PackageSelector) -> Vec<String> {
        let mut argv = vec!["cargo".to_string(), self.cargo_command.to_string()];
        match selector {
            PackageSelector::Inline => {
                if self.supports_color() {
                    argv.push("--color".to_string());
                    argv.push("always".to_string());
                }
                argv.push("--manifest-path".to_string());
                argv.push("{module.manifest}".to_string());
                argv.push("-p".to_string());
                argv.push("{module.package}".to_string());
            }
            PackageSelector::ModuleArgs => {
                argv.push("--manifest-path".to_string());
                argv.push("{module.manifest}".to_string());
                argv.push("{module.args}".to_string());
            }
        }
        argv.extend(self.extra_args.iter().map(ToString::to_string));
        argv.push("{args}".to_string());
        argv
    }

    fn supports_color(&self) -> bool {
        self.cargo_command != "fmt"
    }

    fn shared_inputs(&self, root: &Path, broad: &[String]) -> Vec<String> {
        let mut shared_inputs = broad.to_vec();
        if self.cargo_command == "clippy" {
            shared_inputs.extend(existing_candidates(root, CLIPPY_SHARED_INPUT_CANDIDATES));
        }
        if self.cargo_command == "fmt" {
            shared_inputs.extend(existing_candidates(root, RUSTFMT_SHARED_INPUT_CANDIDATES));
        }
        shared_inputs
    }
}

#[derive(Debug, Clone, Copy)]
enum PackageSelector {
    Inline,
    ModuleArgs,
}

fn existing_shared_inputs(root: &Path) -> Vec<String> {
    existing_candidates(root, BROAD_SHARED_INPUT_CANDIDATES)
}

fn existing_candidates(root: &Path, candidates: &[&str]) -> Vec<String> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| root.join(candidate).is_file())
        .map(str::to_string)
        .collect()
}
