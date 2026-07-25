use std::path::PathBuf;

use rskit_errors::AppResult;
use rskit_testutil::{Match, Normalizer, Rule};

use super::model::{MatcherKind, StreamExpectation};

/// The per-scenario volatile-token scope the default normalizer scrubs.
#[derive(Debug, Clone)]
pub struct NormalizeScope {
    /// The materialized fixture repo root; becomes `<REPO>`.
    pub repo_root: PathBuf,
    /// The scenario-scoped cache directory; becomes `<CACHE>`.
    pub cache_dir: PathBuf,
}

/// Toven's default normalization rule set, defined once for every
/// `match: normalized` stream: temp roots, cache dir, path separators,
/// durations, and content hashes become stable placeholders.
///
/// # Errors
///
/// Returns a typed error if a built-in pattern fails to compile (a defect, not
/// an input condition — covered by tests).
pub fn default_normalizer(scope: &NormalizeScope) -> AppResult<Normalizer> {
    Ok(Normalizer::new(vec![
        // Cache before repo so a cache dir nested under the repo still maps
        // to `<CACHE>` rather than `<REPO>/…`.
        Rule::literal(scope.cache_dir.display().to_string(), "<CACHE>"),
        Rule::literal(scope.repo_root.display().to_string(), "<REPO>"),
        // Windows path separators, after the roots (which contain native ones).
        Rule::literal("\\", "/"),
        Rule::pattern(r"\b[0-9a-f]{64}\b", "<SHA>")?,
        Rule::pattern(r"\b[0-9a-f]{40}\b", "<SHA>")?,
        // The human APPLY summary prints elapsed time as a bare-integer
        // `duration-ms:  N` line (no unit suffix), so the millisecond rule
        // below never sees it; scrub the whole line value here first.
        Rule::pattern(r"duration-ms:  \d+", "duration-ms:  <DUR>")?,
        Rule::pattern(r"\b\d+(?:\.\d+)?(?:ms|s)\b", "<DUR>")?,
    ]))
}

impl StreamExpectation {
    /// Resolve this expectation to an rskit [`Match`] under `scope`.
    ///
    /// Frame fields are validated at load time, so absent frames default to
    /// zero here.
    ///
    /// # Errors
    ///
    /// Propagates [`default_normalizer`]'s pattern-compile error.
    pub fn to_match(&self, scope: &NormalizeScope) -> AppResult<Match> {
        Ok(match self.matcher {
            MatcherKind::Exact => Match::Exact,
            MatcherKind::Normalized => Match::Normalized(default_normalizer(scope)?),
            MatcherKind::LineSet => Match::LineSet {
                frame_prefix: self.frame_prefix.unwrap_or(0),
                frame_suffix: self.frame_suffix.unwrap_or(0),
            },
            MatcherKind::Subset => Match::Subset,
        })
    }
}
