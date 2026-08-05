//! [`ReleasePhase`] — the named stages of a release flow the engine orchestrates.

use serde::{Deserialize, Serialize};

/// A named stage of the release flow.
///
/// A release flow is an ordered set of phases the engine drives — from
/// selecting what to release through publishing provenance. This enum is purely
/// descriptive vocabulary: it *names* the stages so config, reporting, and the
/// per-phase seam can refer to them, but it holds no behavior. The engine
/// already implements most stages as functions; this does not reimplement them.
///
/// The engine owns every phase's flow guarantees (mutation-free preview,
/// `--yes` + allowed branch + clean tree for mutation, immutable outputs with
/// forward-fix recovery, typed JSONL/human reporting) **regardless of how the
/// phase is backed** — see `PhaseBacking` in `toven-ports` for the
/// native-or-delegated backing concept.
///
/// `#[non_exhaustive]` because the flow may grow further phases.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ReleasePhase {
    /// Resolve which modules release and in what dependency order — the
    /// selection + cascade + ordering stage.
    Select,
    /// Decide the next version and update the manifest and changelog — the
    /// version/CHANGELOG decision, before any tag.
    Bump,
    /// Create the release tag under the module's tag grammar.
    Tag,
    /// Build and verify the publishable artifact for the module.
    Package,
    /// Sign the packaged artifact (and, where applicable, the tag).
    Sign,
    /// Publish the version to its registry, once, with classified idempotency.
    Publish,
    /// Cut or reconcile the hosted forge Release and upload its assets.
    Host,
    /// Build a tagged container image, push it to the primary registry plus any
    /// mirrors, and sign the pushed digest.
    Image,
    /// Attach supply-chain provenance and the SBOM to the release.
    Provenance,
}

impl ReleasePhase {
    /// Every phase, in flow order.
    pub const ALL: &'static [Self] = &[
        Self::Select,
        Self::Bump,
        Self::Tag,
        Self::Package,
        Self::Sign,
        Self::Publish,
        Self::Host,
        Self::Image,
        Self::Provenance,
    ];

    /// Canonical config/report name for the phase (stable, matches the serde
    /// representation).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Bump => "bump",
            Self::Tag => "tag",
            Self::Package => "package",
            Self::Sign => "sign",
            Self::Publish => "publish",
            Self::Host => "host",
            Self::Image => "image",
            Self::Provenance => "provenance",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReleasePhase;

    #[test]
    fn all_lists_every_phase_in_flow_order() {
        let names: Vec<&str> = ReleasePhase::ALL
            .iter()
            .map(|phase| phase.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "select",
                "bump",
                "tag",
                "package",
                "sign",
                "publish",
                "host",
                "image",
                "provenance",
            ]
        );
    }

    #[test]
    fn all_is_unique() {
        for (index, phase) in ReleasePhase::ALL.iter().enumerate() {
            assert!(
                !ReleasePhase::ALL[..index].contains(phase),
                "duplicate phase {phase:?}"
            );
        }
    }

    #[test]
    fn names_are_stable_serde() {
        for phase in ReleasePhase::ALL.iter().copied() {
            let json = serde_json::to_string(&phase).expect("serializes");
            assert_eq!(json, format!("\"{}\"", phase.as_str()));
            let back: ReleasePhase = serde_json::from_str(&json).expect("round-trips");
            assert_eq!(back, phase);
        }
    }

    #[test]
    fn unknown_phase_is_a_typed_error() {
        let error = serde_json::from_str::<ReleasePhase>(r#""packaging""#)
            .expect_err("unknown phase rejected");
        assert!(error.to_string().contains("packaging"), "{error}");
    }
}
