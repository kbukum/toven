//! Validation for task `shared_inputs` before they enter the cache key.
//!
//! A task's `shared_inputs` are workspace-relative *literal* file or directory
//! paths that participate in the shared-cache hash. They are hashed verbatim by
//! [`SourceDigest::path`](toven_ports::SourceDigest::path) — no glob expansion,
//! no template substitution — so any pattern or template here would silently
//! hash to an empty digest (a missing file) and break cache correctness.
//!
//! The engine treats adapter- and config-provided `shared_inputs` as untrusted
//! at the PLAN boundary and rejects them up front rather than letting an
//! invalid entry reach the hasher. The relative/traversal/absolute rules are
//! the shared rskit `validate_safe_path` policy; the literal-path rules (no
//! glob or template metacharacters) are Toven's own task-config semantics.

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::validate_safe_path;

/// Characters that mark a path as a glob pattern or an unresolved template
/// rather than a literal relative path. Toven renders templates with `${…}` and
/// never glob-expands `shared_inputs`, so any of these signals a config
/// mistake.
const PATTERN_METACHARACTERS: &[char] = &['*', '?', '[', ']', '{', '}', '$'];

/// Validate a single resolved `shared_inputs` entry for `unit_id`.
///
/// # Errors
/// Rejects absolute paths, empty/`.`/`..` segments, and any glob or template
/// metacharacter, preserving the underlying [`validate_safe_path`] cause.
pub(super) fn validate_shared_input(unit_id: &str, entry: &str) -> AppResult<()> {
    validate_safe_path(entry).map_err(|error| {
        AppError::invalid_input(
            "shared_inputs",
            format!("unit '{unit_id}' shared input '{entry}' is not a safe relative path: {error}"),
        )
        .with_cause(error)
    })?;
    if let Some(found) = entry.chars().find(|ch| PATTERN_METACHARACTERS.contains(ch)) {
        return Err(AppError::invalid_input(
            "shared_inputs",
            format!(
                "unit '{unit_id}' shared input '{entry}' must be a literal relative path, not a \
                 glob or template (found '{found}')"
            ),
        ));
    }
    Ok(())
}

/// Validate every resolved `shared_inputs` entry for `unit_id`.
///
/// # Errors
/// Propagates the first invalid entry from [`validate_shared_input`].
pub(super) fn validate_shared_inputs(unit_id: &str, entries: &[String]) -> AppResult<()> {
    for entry in entries {
        validate_shared_input(unit_id, entry)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_shared_inputs;

    #[test]
    fn accepts_literal_relative_files_and_dirs() {
        assert!(validate_shared_inputs("rust:app#test", &["Cargo.lock".into()]).is_ok());
        assert!(validate_shared_inputs("rust:app#test", &["config/base.toml".into()]).is_ok());
    }

    #[test]
    fn rejects_absolute_paths() {
        assert!(validate_shared_inputs("rust:app#test", &["/etc/passwd".into()]).is_err());
    }

    #[test]
    fn rejects_parent_and_dot_segments() {
        assert!(validate_shared_inputs("rust:app#test", &["../escape".into()]).is_err());
        assert!(validate_shared_inputs("rust:app#test", &["./here".into()]).is_err());
    }

    #[test]
    fn rejects_glob_patterns() {
        assert!(validate_shared_inputs("rust:app#test", &["src/**/*.rs".into()]).is_err());
        assert!(validate_shared_inputs("rust:app#test", &["data/[abc].txt".into()]).is_err());
        assert!(validate_shared_inputs("rust:app#test", &["pkg/{a,b}.toml".into()]).is_err());
    }

    #[test]
    fn rejects_unresolved_templates() {
        assert!(validate_shared_inputs("rust:app#test", &["${module.root}/x".into()]).is_err());
    }
}
