//! Additive, idempotent fragment merge over an existing `toven.toml`.
//!
//! Re-running `toven generate` against a document that already exists grows the
//! polyglot config without disturbing hand edits: a section that is **not yet
//! present** is added, an existing one is **left untouched** with a warning, and
//! `[project]`/`[toven]` are never modified. `--force <id>` regenerates exactly
//! one section. Formatting and comments survive because the edit goes through
//! `toml_edit`, not a destructive re-serialize.

use rskit_errors::{AppError, AppResult, ErrorCode};
use toml_edit::DocumentMut;
use toven_model::EcosystemId;
use toven_ports::EcosystemFragment;

use super::render::insert_section;

/// The outcome of merging fragments into an existing document.
pub(super) struct MergeResult {
    /// The rendered, format-preserving document text.
    pub(super) text: String,
    /// Sections newly added to the document.
    pub(super) added: Vec<EcosystemId>,
    /// Sections regenerated because `--force <id>` named them.
    pub(super) regenerated: Vec<EcosystemId>,
    /// Human-facing diagnostics (an existing section skipped on a plain re-run).
    pub(super) warnings: Vec<String>,
}

/// Merge `fragments` into the existing `existing` document text.
///
/// `force` names the single ecosystem id whose section should be regenerated
/// even when it already exists. Every other already-present section is preserved
/// and reported as a skip warning.
///
/// # Errors
/// Returns an error if `existing` is not valid TOML or a fragment table cannot
/// be re-encoded.
pub(super) fn merge(
    existing: &str,
    fragments: &[EcosystemFragment],
    force: Option<&str>,
) -> AppResult<MergeResult> {
    let mut doc: DocumentMut = existing.parse().map_err(|error| {
        AppError::new(
            ErrorCode::InvalidInput,
            "existing toven.toml is not valid TOML; refusing to merge",
        )
        .with_cause(error)
    })?;

    let mut result = MergeResult {
        text: String::new(),
        added: Vec::new(),
        regenerated: Vec::new(),
        warnings: Vec::new(),
    };

    for fragment in fragments {
        let id = fragment.ecosystem.as_str();
        let present = section_present(&doc, id);
        let forced = force == Some(id);

        if present && !forced {
            result.warnings.push(skip_hint(id));
            continue;
        }

        insert_section(&mut doc, fragment)?;
        if present {
            result.regenerated.push(fragment.ecosystem.clone());
        } else {
            result.added.push(fragment.ecosystem.clone());
        }
    }

    if let Some(id) = force
        && !fragments
            .iter()
            .any(|fragment| fragment.ecosystem.as_str() == id)
    {
        result.warnings.push(force_no_effect_hint(id));
    }

    result.text = doc.to_string();
    Ok(result)
}

/// Whether `[ecosystems.<id>]` already exists in `doc`.
fn section_present(doc: &DocumentMut, id: &str) -> bool {
    doc.get("ecosystems")
        .and_then(toml_edit::Item::as_table_like)
        .is_some_and(|ecosystems| ecosystems.contains_key(id))
}

/// The additive-re-run skip warning for an already-present section.
fn skip_hint(id: &str) -> String {
    format!("[ecosystems.{id}] already exists; skipping (use `--force {id}` to regenerate it)")
}

/// The diagnostic for a `--force <id>` whose ecosystem was never detected.
///
/// Shared by the additive-merge path and the first-run path so the two callers
/// cannot drift.
pub(super) fn force_no_effect_hint(id: &str) -> String {
    format!(
        "--force '{id}' had no effect: no provider or driver detected ecosystem '{id}' under the project root"
    )
}
