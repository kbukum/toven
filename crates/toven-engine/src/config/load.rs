//! Load orchestration: strict parse → structural validation → dispatch check.
//!
//! A thin wrapper over `rskit-config`'s strict loader: the strict loader handles bounded
//! reads, codec decode (honoring `deny_unknown_fields`), verbatim retention of the
//! dynamic-keyed `[ecosystems.<id>]` subtrees, and identity-aware include-merge.
//! This layer contributes only Toven's domain logic — resolving the include list,
//! the structural-validation pass, and the ecosystem-id dispatch check.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rskit_config::{CompositeKey, IdentityKey, IncludeMerge, RawValue, StrictLoader};
use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_validation::input::validate_safe_path;
use toven_model::EcosystemId;

use super::{CanonicalRegistry, Dispatch, Document, dispatch, validate};

/// A successfully loaded `toven.toml`: the strict [`Document`] paired with the
/// ecosystem-id [`Dispatch`] classification computed during the same load.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The strict, structurally-validated document.
    pub document: Document,
    /// The three-way ecosystem-id dispatch outcome (loaded / canonical-unloaded).
    pub dispatch: Dispatch,
}

/// Load, validate, and dispatch the `toven.toml` at `path`.
///
/// Returns the strict [`Document`] and its [`Dispatch`] once the file parses,
/// passes structural validation, and every `[ecosystems.<id>]` section dispatches
/// cleanly (an unknown ecosystem id is a hard error; a canonical-but-unloaded one
/// is accepted and surfaced in [`Dispatch::ignored`]). The dispatch is computed
/// once here so callers reuse it rather than re-classifying. `loaded` is the set
/// of ecosystem ids with an adapter compiled into this binary; `canonical` is the
/// known-ecosystem registry.
pub fn load(
    path: impl AsRef<Path>,
    loaded: &BTreeSet<EcosystemId>,
    canonical: &CanonicalRegistry,
) -> AppResult<Loaded> {
    let path = path.as_ref();
    let document = read_document(path)?;
    validate::structural(&document, canonical)?;
    let dispatch = dispatch::dispatch(&document, loaded, canonical)?;
    Ok(Loaded { document, dispatch })
}

/// Parse the document, merging any `[toven].include` files beneath it.
///
/// The canonical file is read once; its `[toven].include` list is resolved to
/// confined paths and merged beneath the document as defaults (canonical wins on
/// collisions). The merge policy hard-errors on duplicate `[[members]]`,
/// `[[overlays]]`, and `[groups.<name>]` identities across files.
fn read_document(path: &Path) -> AppResult<Document> {
    StrictLoader::new(path)
        .with_merge(include_merge())
        .load_resolving_includes(|canonical| resolve_includes(path, canonical))
}

/// Identity-aware merge policy for the reserved multi-entry sections.
///
/// - `[[members]]` and `[[overlays]]` concatenate across files and hard-error on
///   a duplicate identity (member `name`; the full overlay `from`/`to` edge);
/// - `[groups.<name>]` is a table keyed by group name, so a name contributed by
///   two files is a duplicate identity, not a silent merge.
fn include_merge() -> IncludeMerge {
    IncludeMerge::new()
        .with_identity("members", IdentityKey::new("name"))
        .with_identity(
            "overlays",
            CompositeKey::new(["from.ecosystem", "from.module", "to.ecosystem", "to.module"]),
        )
        .with_unique_keys("groups")
}

/// Resolve the workspace-relative `[toven].include` list to confined paths.
fn resolve_includes(path: &Path, raw: &RawValue) -> AppResult<Vec<PathBuf>> {
    let Some(entries) = raw.get("toven").and_then(|toven| toven.get("include")) else {
        return Ok(Vec::new());
    };
    let entries = entries.as_array().ok_or_else(|| {
        AppError::invalid_input("toven.include", "must be an array of relative file paths")
    })?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut resolved = Vec::with_capacity(entries.len());
    for entry in entries {
        let relative = entry.as_str().ok_or_else(|| {
            AppError::invalid_input("toven.include", "include entries must be strings")
        })?;
        validate_safe_path(relative)
            .map_err(|error| AppError::invalid_input("toven.include", error.to_string()))?;
        let joined = safe_join(base, relative)
            .map_err(|error| AppError::invalid_input("toven.include", error.to_string()))?;
        resolved.push(joined);
    }
    Ok(resolved)
}
