//! Member identity stamping for the cross-repo union.
//!
//! Member is **metadata** on [`Module`], not part of two-level `ecosystem:name`
//! identity. After each member's discovery output is unioned into the one
//! federation, every module discovered under a declared member is stamped with
//! that member's id so [`Module::key`](toven_model::Module::key) yields a
//! member-scoped [`ModuleKey`](toven_model::ModuleKey).
//!
//! Stamping is what makes the model `member` slot load-bearing and what lets the
//! union build at all: two members that each expose `rust:core` would otherwise
//! produce a duplicate identity that [`Graph::build`](toven_model::Graph::build)
//! rejects. Once stamped, their keys differ (`core/rust:core` vs
//! `gateway/rust:core`). Auto-qualify-only-on-collision is not a stamping concern:
//! every umbrella module carries its member qualifier on the key, and dropping the
//! qualifier when a reference is unambiguous lives in graph resolution/display
//! (`plan::graph`), never in the key itself.
//!
//! The degenerate single-repo member (a lone `[project]` with no `[[members]]`)
//! has no member id, so its modules are left unstamped and the single-repo path
//! stays byte-for-byte identical.

use toven_model::{MemberId, Module};

/// Stamp every discovered module under a member with that member's id.
///
/// Modules already carrying a different member are overwritten: the umbrella owns
/// the member dimension for the union, and a member adapter never sets it.
pub(crate) fn stamp_modules(modules: &mut [Module], member: &MemberId) {
    for module in modules {
        module.member = Some(member.clone());
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{EcosystemId, MemberId, Module, ModuleRef, RepoPath};

    use super::stamp_modules;

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            RepoPath::new(name).unwrap(),
        )
    }

    #[test]
    fn stamping_sets_member_on_every_module() {
        let member = MemberId::new("billing").unwrap();
        let mut modules = vec![module("core"), module("api")];

        stamp_modules(&mut modules, &member);

        assert!(modules.iter().all(|m| m.member.as_ref() == Some(&member)));
    }

    #[test]
    fn stamping_makes_colliding_keys_distinct() {
        let billing = MemberId::new("billing").unwrap();
        let gateway = MemberId::new("gateway").unwrap();
        let mut billing_modules = vec![module("core")];
        let mut gateway_modules = vec![module("core")];

        // Two members each expose `rust:core`; pre-stamp their keys collide.
        assert_eq!(billing_modules[0].key(), gateway_modules[0].key());

        stamp_modules(&mut billing_modules, &billing);
        stamp_modules(&mut gateway_modules, &gateway);

        assert_ne!(billing_modules[0].key(), gateway_modules[0].key());
        assert_eq!(billing_modules[0].key().to_string(), "billing/rust:core");
        assert_eq!(gateway_modules[0].key().to_string(), "gateway/rust:core");
    }

    #[test]
    fn stamping_overwrites_any_pre_existing_member() {
        let stale = MemberId::new("stale").unwrap();
        let owner = MemberId::new("owner").unwrap();
        let mut modules = vec![module("core")];
        modules[0].member = Some(stale);

        stamp_modules(&mut modules, &owner);

        assert_eq!(modules[0].member.as_ref(), Some(&owner));
    }
}
