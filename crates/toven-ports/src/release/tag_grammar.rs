//! [`TagGrammar`] — the tag-scheme phase contract (`tag`).

use rskit_errors::AppResult;
use toven_model::Module;

use super::TagScheme;

/// Build a module's release-tag grammar.
///
/// The `tag` phase's ecosystem sliver: the engine owns when and whether a tag
/// is cut, signed, and pushed; this port only names the tag shape. Object-safe
/// so the engine can hold it behind [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait TagGrammar {
    /// Build this module's release-tag scheme, honoring a configured
    /// `tag_format` override (`None` = the ecosystem-default shape).
    fn tag_scheme(&self, module: &Module, tag_format: Option<&str>) -> AppResult<TagScheme>;
}
