//! `BaselineStrategy` — the engine-owned named policy that resolves CLI flags
//! and `[project].base_ref` into a typed [`BaselineSpec`].
//!
//! Consistent with the other engine-owned named policies (`RunStrategy`,
//! `ReleaseStrategy`): the git mechanism
//! ([`VcsReader`](toven_ports::VcsReader)) stays policy-free and consumes the
//! resolved spec, while the *which-ref / merge-base* decision lives here — pure
//! and unit-testable without a repo.

use rskit_errors::{AppError, AppResult};
use toven_ports::BaselineSpec;

/// CLI-sourced baseline selection, populated by the argv layer.
///
/// `base` (`--base <ref>`) overrides `[project].base_ref` as the reference;
/// `merge_base` (`--merge-base`) is an orthogonal modifier choosing
/// `merge-base(reference, HEAD)` over the reference directly. Keeping reference
/// and mode separate mirrors [`BaselineSpec`].
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct BaselineFlags {
    /// `--base <ref>`: overrides `[project].base_ref` as the baseline
    /// reference.
    pub base: Option<String>,
    /// `--merge-base`: diff against `merge-base(reference, HEAD)`.
    pub merge_base: bool,
}

impl BaselineFlags {
    /// Empty flags (no `--base`, no `--merge-base`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the `--base <ref>` reference override.
    #[must_use]
    pub fn with_base(mut self, reference: impl Into<String>) -> Self {
        self.base = Some(reference.into());
        self
    }

    /// Select `--merge-base` mode.
    #[must_use]
    pub const fn with_merge_base(mut self, merge_base: bool) -> Self {
        self.merge_base = merge_base;
        self
    }
}

/// The engine-owned named baseline policy.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BaselineStrategy;

impl BaselineStrategy {
    /// Resolve CLI flags and the configured `[project].base_ref` into a typed
    /// [`BaselineSpec`].
    ///
    /// Precedence for the reference is `--base` then `[project].base_ref`; the
    /// mode is `MergeBase` iff `--merge-base` is set, else `Explicit`. Errors
    /// when no reference is available from either source — argv is never
    /// silently rewritten with a hidden default.
    pub fn resolve(
        flags: &BaselineFlags,
        project_base_ref: Option<&str>,
    ) -> AppResult<BaselineSpec> {
        Self::resolve_optional(flags, project_base_ref).ok_or_else(|| {
            AppError::invalid_input(
                "base_ref",
                "no baseline reference: pass --base <ref> or set [project].base_ref / [[members]].base_ref",
            )
        })
    }

    /// Resolve like [`resolve`](Self::resolve) but yield `None` when neither
    /// the flags nor config name a reference, instead of erroring.
    ///
    /// Opening the per-member reader set must not force a baseline: a
    /// single-repo `build` planning every module (`Selection::All`) never
    /// consults one, so the missing-reference error belongs at the point a
    /// changed-selection actually consumes the baseline — not at open time.
    #[must_use]
    pub fn resolve_optional(
        flags: &BaselineFlags,
        project_base_ref: Option<&str>,
    ) -> Option<BaselineSpec> {
        let reference = flags.base.as_deref().or(project_base_ref)?;
        Some(if flags.merge_base {
            BaselineSpec::merge_base(reference)
        } else {
            BaselineSpec::explicit(reference)
        })
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::BaselineMode;

    use super::{BaselineFlags, BaselineStrategy};

    #[test]
    fn explicit_base_flag_overrides_config() {
        let flags = BaselineFlags::new().with_base("feature");
        let spec = BaselineStrategy::resolve(&flags, Some("origin/main")).expect("resolves");
        assert_eq!(spec.reference, "feature");
        assert_eq!(spec.mode, BaselineMode::Explicit);
    }

    #[test]
    fn config_base_ref_used_when_no_flag() {
        let spec = BaselineStrategy::resolve(&BaselineFlags::new(), Some("origin/main"))
            .expect("resolves");
        assert_eq!(spec.reference, "origin/main");
        assert_eq!(spec.mode, BaselineMode::Explicit);
    }

    #[test]
    fn merge_base_flag_selects_merge_base_mode() {
        let flags = BaselineFlags::new().with_merge_base(true);
        let spec = BaselineStrategy::resolve(&flags, Some("origin/main")).expect("resolves");
        assert_eq!(spec.reference, "origin/main");
        assert_eq!(spec.mode, BaselineMode::MergeBase);
    }

    #[test]
    fn missing_reference_is_an_error() {
        let error = BaselineStrategy::resolve(&BaselineFlags::new(), None)
            .expect_err("no reference available");
        assert!(error.message().contains("no baseline reference"));
        assert!(error.message().contains("[[members]].base_ref"));
    }
}
