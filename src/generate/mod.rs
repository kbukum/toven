//! User-facing config generation workflow.

mod cli;
mod global;
mod model;
mod render;
mod writer;

use std::path::PathBuf;

use crate::{
    adapter::rust::generate::RustGenerateContributor,
    core::{AdapterId, AppError, AppResult},
};

pub use cli::{GenerateCliOptions, run_generate};
pub use model::{
    GenerateContext, GenerateContributor, GenerateDocument, GenerateRequest, GeneratedProfile,
    TomlValue,
};
pub use render::render_document;
pub use writer::write_document;

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
            document.profiles.insert(profile.name.clone(), profile);
        }
    }
    if document.profiles.is_empty() {
        return Err(AppError::invalid_input(
            "generate",
            "no supported project manifests found; pass --manifest for Rust projects without a root Cargo.toml",
        ));
    }

    let rendered = render::render_document(&document)?;
    if request.write {
        writer::write_document(&context.root, &rendered, request.overwrite)?;
    }

    Ok(GenerateOutcome { document, rendered })
}

/// Result of a generation run.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GenerateOutcome {
    /// Structured generated document.
    pub document: GenerateDocument,
    /// Rendered TOML.
    pub rendered: String,
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

pub(crate) fn path_string(path: &std::path::Path) -> String {
    render::path_string(path)
}
