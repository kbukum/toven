//! Graph node key: a [`ModuleRef`] optionally scoped to a federation member.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{MemberId, ModuleRef};

/// The key a [`Graph`](crate::Graph) indexes a module by.
///
/// Module *identity* stays two-level `ecosystem:name` ([`ModuleRef`]); the
/// optional `member` qualifier disambiguates the same `ecosystem:name` exposed
/// by two different members of a cross-repo umbrella. The `member` is `None` for
/// the single-repo case, so a bare key renders and orders byte-for-byte
/// identically to its underlying `ModuleRef` — the degenerate path is unchanged.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
pub struct ModuleKey {
    /// Federation member that owns the module, when member scoping is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberId>,
    /// The two-level module identity within its member.
    pub module: ModuleRef,
}

impl ModuleKey {
    /// Construct a member-scoped key (`member = None` is the single-repo case).
    #[must_use]
    pub const fn new(member: Option<MemberId>, module: ModuleRef) -> Self {
        Self { member, module }
    }

    /// Construct an unscoped (single-repo / non-colliding) key.
    #[must_use]
    pub const fn bare(module: ModuleRef) -> Self {
        Self {
            member: None,
            module,
        }
    }

    /// The owning federation member, when the key is member-scoped.
    #[must_use]
    pub const fn member(&self) -> Option<&MemberId> {
        self.member.as_ref()
    }

    /// The underlying two-level module identity.
    #[must_use]
    pub const fn module(&self) -> &ModuleRef {
        &self.module
    }

    /// Return this key scoped to `member`, replacing any existing qualifier.
    #[must_use]
    pub fn with_member(mut self, member: MemberId) -> Self {
        self.member = Some(member);
        self
    }
}

impl From<ModuleRef> for ModuleKey {
    fn from(module: ModuleRef) -> Self {
        Self::bare(module)
    }
}

impl fmt::Display for ModuleKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.member {
            Some(member) => write!(formatter, "{member}/{}", self.module),
            None => write!(formatter, "{}", self.module),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MemberId, ModuleKey, ModuleRef};
    use crate::identity::EcosystemId;

    fn module_ref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    #[test]
    fn bare_key_renders_like_its_module_ref() {
        let key = ModuleKey::bare(module_ref("rust", "core"));
        assert_eq!(key.to_string(), "rust:core");
        assert_eq!(key, ModuleKey::from(module_ref("rust", "core")));
    }

    #[test]
    fn scoped_key_renders_member_prefix() {
        let key = ModuleKey::bare(module_ref("rust", "core"))
            .with_member(MemberId::new("billing").unwrap());
        assert_eq!(key.to_string(), "billing/rust:core");
    }

    #[test]
    fn member_scoping_distinguishes_same_module_ref() {
        let bare = ModuleKey::bare(module_ref("rust", "core"));
        let billing = bare.clone().with_member(MemberId::new("billing").unwrap());
        let gateway = bare.with_member(MemberId::new("gateway").unwrap());
        assert_ne!(billing, gateway);
    }
}
