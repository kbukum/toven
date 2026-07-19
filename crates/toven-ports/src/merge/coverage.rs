//! Field-merge: fold a per-module coverage override onto an ecosystem-default
//! [`CoverageConfig`].

use crate::config::CoverageConfig;

/// Field-merge a per-module coverage `over`ride onto an ecosystem `base`
/// config.
///
/// Every field is presence-aware: a `Some` override threshold/enforcement
/// **replaces** the base value for exactly that field, and a `None` inherits
/// the base. A non-empty override `exclude`/`profiles` replaces the base
/// list/map; an empty one inherits (a per-module override rarely re-declares
/// them). This matches the documented precedence (per-module > profile >
/// ecosystem > adapter default), with profile resolution layered in the engine
/// between the ecosystem default and this per-module override.
#[must_use]
pub fn merge_coverage(base: &CoverageConfig, over: &CoverageConfig) -> CoverageConfig {
    let mut merged = base.clone();

    if over.line.is_some() {
        merged.line = over.line;
    }
    if over.function.is_some() {
        merged.function = over.function;
    }
    if over.region.is_some() {
        merged.region = over.region;
    }
    if over.changed_line.is_some() {
        merged.changed_line = over.changed_line;
    }
    if over.enforcement.is_some() {
        merged.enforcement = over.enforcement;
    }
    if !over.exclude.is_empty() {
        merged.exclude.clone_from(&over.exclude);
    }
    if !over.profiles.is_empty() {
        merged.profiles.clone_from(&over.profiles);
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::merge_coverage;
    use crate::config::{CoverageConfig, Enforcement};

    fn base() -> CoverageConfig {
        CoverageConfig {
            line: Some(90.0),
            function: Some(85.0),
            enforcement: Some(Enforcement::Block),
            exclude: vec!["toven-suite".into()],
            ..CoverageConfig::default()
        }
    }

    #[test]
    fn set_override_field_replaces_and_rest_inherits() {
        let over = CoverageConfig {
            line: Some(80.0),
            enforcement: Some(Enforcement::Advisory),
            ..CoverageConfig::default()
        };

        let merged = merge_coverage(&base(), &over);

        assert_eq!(merged.line, Some(80.0));
        assert_eq!(merged.enforcement, Some(Enforcement::Advisory));
        // inherited from base, untouched by the override:
        assert_eq!(merged.function, Some(85.0));
        assert_eq!(merged.exclude, ["toven-suite"]);
    }

    #[test]
    fn empty_override_inherits_base_entirely() {
        let merged = merge_coverage(&base(), &CoverageConfig::default());
        assert_eq!(merged, base());
    }

    #[test]
    fn non_empty_override_list_replaces_base_list() {
        let over = CoverageConfig {
            exclude: vec!["fixtures".into()],
            ..CoverageConfig::default()
        };

        let merged = merge_coverage(&base(), &over);

        assert_eq!(merged.exclude, ["fixtures"]);
    }
}
