//! Per-member ecosystem scope: an [`EcosystemId`] optionally qualified by the
//! federation member that owns it.

use super::{EcosystemId, MemberId};

/// The scope a per-ecosystem runtime setting (e.g. a compute budget) applies to.
///
/// An ecosystem's *identity* is [`EcosystemId`]; the optional `member`
/// qualifier disambiguates the same ecosystem (`go`) configured independently
/// by two different members of a cross-repo umbrella, each carrying its own
/// `[ecosystems.go]`. `member` is `None` for the single-repo case, so a bare
/// scope orders and resolves exactly like a plain [`EcosystemId`] lookup — the
/// degenerate path is unchanged.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct EcosystemScope {
    /// Federation member that owns the ecosystem config, when member scoping is
    /// required.
    member: Option<MemberId>,
    /// The ecosystem this scope addresses.
    ecosystem: EcosystemId,
}

impl EcosystemScope {
    /// Construct a member-scoped ecosystem scope (`member = None` is the
    /// single-repo case).
    #[must_use]
    pub const fn new(member: Option<MemberId>, ecosystem: EcosystemId) -> Self {
        Self { member, ecosystem }
    }

    /// Construct an unscoped (single-repo / non-colliding) ecosystem scope.
    #[must_use]
    pub const fn bare(ecosystem: EcosystemId) -> Self {
        Self {
            member: None,
            ecosystem,
        }
    }

    /// The owning federation member, when the scope is member-scoped.
    #[must_use]
    pub const fn member(&self) -> Option<&MemberId> {
        self.member.as_ref()
    }

    /// The addressed ecosystem.
    #[must_use]
    pub const fn ecosystem(&self) -> &EcosystemId {
        &self.ecosystem
    }
}

#[cfg(test)]
mod tests {
    use super::{EcosystemId, EcosystemScope, MemberId};

    fn go() -> EcosystemId {
        EcosystemId::new("go").expect("valid id")
    }

    #[test]
    fn bare_scope_has_no_member() {
        let scope = EcosystemScope::bare(go());
        assert_eq!(scope.member(), None);
        assert_eq!(scope.ecosystem(), &go());
    }

    #[test]
    fn member_scope_carries_its_owner() {
        let member = MemberId::new("services").expect("valid id");
        let scope = EcosystemScope::new(Some(member.clone()), go());
        assert_eq!(scope.member(), Some(&member));
        assert_eq!(scope.ecosystem(), &go());
    }

    #[test]
    fn two_members_with_the_same_ecosystem_are_distinct_keys() {
        let core = EcosystemScope::new(Some(MemberId::new("core").expect("id")), go());
        let services = EcosystemScope::new(Some(MemberId::new("services").expect("id")), go());
        let bare = EcosystemScope::bare(go());
        assert_ne!(core, services);
        assert_ne!(core, bare);
    }
}
