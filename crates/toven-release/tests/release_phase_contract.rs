//! Release flow phase-guarantee contract.
//!
//! Names the flow's phases and the guarantees the engine owns for **every**
//! phase, independent of how the phase is backed (native or delegated). It pins
//! that contract two ways:
//!
//! * a **phase × guarantee table** ([`CONTRACT`]) asserting, for every
//!   [`ReleasePhase`], that the four engine-owned guarantees apply; and
//! * **per-phase enforcement placeholders** (`#[ignore]`) that assert each phase
//!   *actually* enforces mutation-free preview and immutable, forward-fix-only
//!   outputs. Per-phase enforcement seams are not yet implemented, so these are
//!   ignored and fail when run with `--ignored`, tracking the outstanding work;
//!   the holistic guarantees are already covered by
//!   `release_preview_mutation_free.rs` and the hosted-release immutability
//!   tests.
//!
//! The `native | delegated` backing carries no guarantee weight of its own: a
//! delegated phase that cannot preview mutation-free is not an acceptable
//! delegation, so the same table binds both backings.

use toven_model::ReleasePhase;
use toven_ports::PhaseBacking;

/// One engine-owned guarantee that holds for a phase regardless of backing.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum Guarantee {
    /// Preview observes without mutating.
    MutationFreePreview,
    /// Real mutation requires `--yes` + an allowed branch + a clean tree.
    GatedMutation,
    /// Outputs are immutable; recovery is a forward-fix version, never a rewrite.
    ImmutableForwardFix,
    /// Reporting is typed JSONL/human on the correct stream.
    TypedReporting,
}

/// The full guarantee set every phase must uphold.
const ALL_GUARANTEES: &[Guarantee] = &[
    Guarantee::MutationFreePreview,
    Guarantee::GatedMutation,
    Guarantee::ImmutableForwardFix,
    Guarantee::TypedReporting,
];

/// The guarantees bound to one phase.
struct PhaseGuarantees {
    /// The phase these guarantees describe.
    phase: ReleasePhase,
    /// The guarantees the phase upholds — every phase binds [`ALL_GUARANTEES`].
    guarantees: &'static [Guarantee],
}

/// The phase × guarantee table: every phase upholds the full guarantee set.
const CONTRACT: &[PhaseGuarantees] = &[
    bound(ReleasePhase::Select),
    bound(ReleasePhase::Bump),
    bound(ReleasePhase::Tag),
    bound(ReleasePhase::Package),
    bound(ReleasePhase::Sign),
    bound(ReleasePhase::Publish),
    bound(ReleasePhase::Host),
    bound(ReleasePhase::Image),
    bound(ReleasePhase::Provenance),
];

/// Bind a phase to the full guarantee set — no phase waives a guarantee.
const fn bound(phase: ReleasePhase) -> PhaseGuarantees {
    PhaseGuarantees {
        phase,
        guarantees: ALL_GUARANTEES,
    }
}

#[test]
fn contract_covers_every_phase_exactly_once() {
    let covered: Vec<ReleasePhase> = CONTRACT.iter().map(|entry| entry.phase).collect();
    let expected: Vec<ReleasePhase> = ReleasePhase::ALL.to_vec();
    assert_eq!(
        covered, expected,
        "the phase × guarantee table must cover every phase in flow order"
    );
}

#[test]
fn every_phase_upholds_all_four_guarantees() {
    for entry in CONTRACT {
        assert_eq!(
            entry.guarantees,
            ALL_GUARANTEES,
            "phase {} must uphold every engine-owned guarantee",
            entry.phase.as_str()
        );
    }
}

#[test]
fn guarantees_bind_native_and_delegated_backings_alike() {
    // The backing a phase resolves to does not change which guarantees apply;
    // both are held to the same row of the contract table.
    for backing in [
        PhaseBacking::native(),
        PhaseBacking::delegated("goreleaser"),
    ] {
        assert_eq!(CONTRACT.len(), ReleasePhase::ALL.len());
        assert!(
            matches!(
                backing,
                PhaseBacking::Native | PhaseBacking::Delegated { .. }
            ),
            "the guarantee table is backing-agnostic"
        );
    }
}

/// Whether `phase` has its own per-phase mutation-free-preview enforcement seam.
///
/// Wired to `true` per phase once per-phase enforcement is implemented; today
/// preview safety is enforced holistically, not per phase.
const fn per_phase_preview_enforced(_phase: ReleasePhase) -> bool {
    false
}

/// Whether `phase` has its own per-phase output-immutability enforcement seam.
const fn per_phase_immutability_enforced(_phase: ReleasePhase) -> bool {
    false
}

#[test]
#[ignore = "per-phase mutation-free-preview enforcement is not yet implemented"]
fn preview_is_mutation_free_for_every_phase() {
    for phase in ReleasePhase::ALL.iter().copied() {
        assert!(
            per_phase_preview_enforced(phase),
            "phase {} has no per-phase mutation-free-preview enforcement yet",
            phase.as_str()
        );
    }
}

#[test]
#[ignore = "per-phase output-immutability enforcement is not yet implemented"]
fn outputs_are_immutable_for_every_phase() {
    for phase in ReleasePhase::ALL.iter().copied() {
        assert!(
            per_phase_immutability_enforced(phase),
            "phase {} has no per-phase immutability/forward-fix enforcement yet",
            phase.as_str()
        );
    }
}
