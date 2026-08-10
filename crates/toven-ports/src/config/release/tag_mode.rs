//! [`TagMode`] — the explicit tag layout a release train creates.
//!
//! A release train's tag layout is otherwise an emergent consequence of each
//! module's `tag_format` plus which module is the `umbrella`. `TagMode` names
//! the choice directly: per-module tags, a single umbrella tag, or both. It
//! governs *what tags are created*; the `umbrella` flag still marks *which*
//! module is the umbrella (its tag is the umbrella tag), and the `baseline`
//! source governs *what change-gating diffs against* — the three are
//! orthogonal.

use serde::{Deserialize, Serialize};

/// Which release tags a train creates.
///
/// The variants compose with the `umbrella` marker: [`Umbrella`](Self::Umbrella)
/// and [`Both`](Self::Both) require exactly one umbrella module per member (its
/// tag is the umbrella tag), validated at plan time. [`PerModule`](Self::PerModule)
/// needs no umbrella module.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum TagMode {
    /// Create one tag per released module from each module's own tag scheme —
    /// the per-module-tag layout (Go's mandatory model, where a module's tag
    /// *is* its registry entry). The umbrella module's own tag is **not**
    /// created in this mode.
    #[default]
    PerModule,
    /// Create only the single umbrella module's tag (e.g. `v1.2.3`), skipping
    /// per-module tags — the workspace-shared layout where the whole train
    /// releases under one repo tag.
    Umbrella,
    /// Create per-module tags **and** the umbrella tag — a per-module tag for
    /// traceability plus one aggregate repo tag.
    Both,
}

impl TagMode {
    /// The stable lowercase identifier used in diagnostics and projections.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PerModule => "per-module",
            Self::Umbrella => "umbrella",
            Self::Both => "both",
        }
    }

    /// Whether this mode creates the umbrella module's tag.
    #[must_use]
    pub const fn creates_umbrella_tag(self) -> bool {
        matches!(self, Self::Umbrella | Self::Both)
    }

    /// Whether this mode creates per-module (non-umbrella) tags.
    #[must_use]
    pub const fn creates_per_module_tags(self) -> bool {
        matches!(self, Self::PerModule | Self::Both)
    }

    /// Whether this mode requires the member to declare exactly one umbrella
    /// module (its tag is the umbrella tag).
    #[must_use]
    pub const fn requires_umbrella(self) -> bool {
        self.creates_umbrella_tag()
    }
}

#[cfg(test)]
mod tests {
    use super::TagMode;

    #[test]
    fn parses_kebab_case_variants() {
        #[derive(serde::Deserialize)]
        struct Wrap {
            mode: TagMode,
        }
        let parse = |value: &str| -> TagMode {
            toml::from_str::<Wrap>(&format!("mode = \"{value}\""))
                .expect("parses")
                .mode
        };
        assert_eq!(parse("per-module"), TagMode::PerModule);
        assert_eq!(parse("umbrella"), TagMode::Umbrella);
        assert_eq!(parse("both"), TagMode::Both);
    }

    #[test]
    fn rejects_unknown_variant() {
        #[derive(serde::Deserialize)]
        struct Wrap {
            #[allow(dead_code)]
            mode: TagMode,
        }
        assert!(toml::from_str::<Wrap>("mode = \"per_module\"").is_err());
    }

    #[test]
    fn tag_selection_predicates_match_each_mode() {
        assert!(TagMode::PerModule.creates_per_module_tags());
        assert!(!TagMode::PerModule.creates_umbrella_tag());
        assert!(!TagMode::PerModule.requires_umbrella());

        assert!(!TagMode::Umbrella.creates_per_module_tags());
        assert!(TagMode::Umbrella.creates_umbrella_tag());
        assert!(TagMode::Umbrella.requires_umbrella());

        assert!(TagMode::Both.creates_per_module_tags());
        assert!(TagMode::Both.creates_umbrella_tag());
        assert!(TagMode::Both.requires_umbrella());
    }
}
