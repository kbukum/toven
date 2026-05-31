//! Rust config generation contributor.

use std::{collections::BTreeMap, path::Path};

use crate::{
    adapter::rust::generate::cargo,
    core::{AdapterId, AppResult, ExecutionMode},
    generate::{GenerateContext, GenerateContributor, GeneratedProfile, TomlValue},
};

/// Generates Rust/Cargo profile fragments.
pub struct RustGenerateContributor {
    adapter_id: AdapterId,
}

impl RustGenerateContributor {
    /// Create a Rust generation contributor.
    pub fn new() -> AppResult<Self> {
        Ok(Self {
            adapter_id: AdapterId::new("rust")?,
        })
    }
}

impl GenerateContributor for RustGenerateContributor {
    fn adapter_id(&self) -> &AdapterId {
        &self.adapter_id
    }

    fn generate(&self, context: &mut GenerateContext) -> AppResult<Option<GeneratedProfile>> {
        let Some(manifests) = cargo::resolve_manifests(&context.root, &context.manifests)? else {
            return Ok(None);
        };

        let mut discovery = BTreeMap::new();
        if manifests != [std::path::PathBuf::from("Cargo.toml")] {
            discovery.insert(
                "manifests".to_string(),
                TomlValue::Array(
                    manifests
                        .iter()
                        .map(|manifest| TomlValue::String(path_string(manifest)))
                        .collect(),
                ),
            );
        }

        Ok(Some(GeneratedProfile {
            name: context.profile_name.clone(),
            adapter: self.adapter_id.clone(),
            execution: ExecutionMode::SpawnEach,
            module_arg_template: vec!["-p".to_string(), "{module.package}".to_string()],
            resource_group: "cargo:{project.root}".to_string(),
            discovery,
        }))
    }
}

fn path_string(path: &Path) -> String {
    let normalized = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}
