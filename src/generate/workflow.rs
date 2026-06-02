//! Config generation workflow orchestration.

use std::path::PathBuf;

use crate::{
    adapter::rust::generate::RustGenerateContributor,
    core::{AdapterId, AppError, AppResult},
    generate::{
        GenerateContext, GenerateContributor, GenerateDocument, GenerateRequest, GeneratedProfile,
        global, render, writer,
    },
};

/// Generate a Toven config and optionally write it to disk.
pub fn generate_config(request: &GenerateRequest) -> AppResult<GenerateOutcome> {
    let root = global::normalize_root(&request.root)?;
    let mut context = GenerateContext {
        root,
        profile_name: request.profile_name.clone(),
        manifests: request.manifests.clone(),
    };

    let contributors = default_contributors()?;
    let selected = select_contributors(&contributors, request.adapter.as_ref())?;
    let mut document = global::base_document(&context)?;
    for contributor in selected {
        if let Some(profile) = contributor.generate(&mut context)? {
            insert_profile(&mut document, contributor.adapter_id(), profile)?;
        }
    }
    if document.profiles.is_empty() {
        return Err(AppError::invalid_input(
            "generate",
            no_match_message(&context, request.adapter.as_ref()),
        ));
    }

    let rendered = render::render_document(&document)?;
    if request.write {
        writer::write_document(&context.root, &rendered, request.overwrite)?;
    }

    Ok(GenerateOutcome { document, rendered })
}

fn no_match_message(context: &GenerateContext, adapter: Option<&AdapterId>) -> String {
    let adapter = adapter
        .map(|adapter| format!(" adapter '{adapter}'"))
        .unwrap_or_else(|| " any supported adapter".to_string());
    format!(
        "no supported project manifests found under '{}' for{adapter}; Rust generation searches for a root Cargo.toml or top-level nested Cargo.toml files. Pass --manifest path/to/Cargo.toml to provide Cargo workspace manifests explicitly.",
        context.root.display()
    )
}

fn insert_profile(
    document: &mut GenerateDocument,
    contributor: &AdapterId,
    profile: GeneratedProfile,
) -> AppResult<()> {
    if document.profiles.contains_key(&profile.name) {
        return Err(AppError::invalid_input(
            "generate.profile",
            format!(
                "multiple generation contributors produced profile '{}'; pass --adapter to generate one adapter at a time or use a unique --profile",
                profile.name
            ),
        ));
    }
    if profile.adapter != *contributor {
        return Err(AppError::invalid_input(
            "generate.adapter",
            format!(
                "contributor '{contributor}' produced profile '{}' for adapter '{}'",
                profile.name, profile.adapter
            ),
        ));
    }
    document.profiles.insert(profile.name.clone(), profile);
    Ok(())
}

/// Result of a generation run.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GenerateOutcome {
    /// Structured generated document.
    pub document: GenerateDocument,
    /// Rendered TOML.
    pub rendered: String,
}

/// Build a request with defaults for callers that do not go through clap.
#[must_use]
pub fn request(root: PathBuf) -> GenerateRequest {
    GenerateRequest {
        root,
        profile_name: "main".to_string(),
        adapter: None,
        manifests: Vec::new(),
        write: false,
        overwrite: false,
    }
}

fn default_contributors() -> AppResult<Vec<Box<dyn GenerateContributor>>> {
    Ok(vec![Box::new(RustGenerateContributor::new()?)])
}

fn select_contributors<'a>(
    contributors: &'a [Box<dyn GenerateContributor>],
    adapter: Option<&AdapterId>,
) -> AppResult<Vec<&'a dyn GenerateContributor>> {
    if let Some(adapter) = adapter {
        let contributor = contributors
            .iter()
            .find(|contributor| contributor.adapter_id() == adapter)
            .ok_or_else(|| {
                AppError::invalid_input(
                    "generate.adapter",
                    format!("unsupported adapter '{adapter}'"),
                )
            })?;
        return Ok(vec![contributor.as_ref()]);
    }

    Ok(contributors
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use crate::{
        core::{AdapterId, ExecutionMode},
        generate::{
            GenerateDocument, GeneratedProfile,
            model::{GeneratedProject, TomlValue},
        },
    };

    use super::insert_profile;

    #[test]
    fn rejects_duplicate_generated_profile_names() {
        let rust = AdapterId::new("rust").expect("rust adapter");
        let other = AdapterId::new("other").expect("other adapter");
        let mut document = GenerateDocument {
            project: GeneratedProject {
                schema: 1,
                name: "demo".to_string(),
                root: PathBuf::from("."),
                base_ref: None,
            },
            profiles: BTreeMap::new(),
            warnings: Vec::new(),
        };
        insert_profile(&mut document, &rust, profile("main", rust.clone())).expect("insert first");

        let error = insert_profile(&mut document, &other, profile("main", other.clone()))
            .expect_err("duplicate profile should fail");

        assert!(error.message.contains("multiple generation contributors"));
    }

    fn profile(name: &str, adapter: AdapterId) -> GeneratedProfile {
        GeneratedProfile {
            name: name.to_string(),
            adapter,
            execution: ExecutionMode::SpawnEach,
            module_arg_template: vec!["-p".to_string(), "{module.package}".to_string()],
            resource_group: "cargo:{project.root}".to_string(),
            tasks: BTreeMap::new(),
            discovery: BTreeMap::<String, TomlValue>::new(),
        }
    }
}
