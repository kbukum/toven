//! Member composition for the cross-repo umbrella.
//!
//! Loads each umbrella member's own `toven.toml` and pairs it with the
//! cross-member overlays/groups the umbrella layers on top.
//!
//! Every member is an
//! independently-runnable toven project that carries its own authoritative
//! configuration, so the umbrella **composes** members (reusing the single strict
//! engine [`load`] — no second parse path) rather than
//! inlining their config. The umbrella file contributes only cross-member
//! `[[overlays]]`/`[groups]` and umbrella-level `[toven]` run knobs; it never
//! rewrites a member's ecosystem config. A declared member without its own
//! `toven.toml` is a hard config error.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_validation::input::validate_safe_path;
use toven_model::{AbsPath, EcosystemId};

use super::members::ResolvedMember;
use crate::config::{CanonicalRegistry, Document, GroupConfig, OverlayConfig, load};

/// The canonical member config filename loaded at each member root.
const MEMBER_CONFIG_FILENAME: &str = "toven.toml";

/// One composed member: its resolved identity, its own authoritative
/// [`Document`], and the absolute root discovery points at.
#[derive(Debug, Clone)]
pub struct ComposedMember {
    member: ResolvedMember,
    document: Document,
    discover_root: AbsPath,
    base_ref: Option<String>,
}

impl ComposedMember {
    /// The resolved member (identity, repo root, declared baseline).
    #[must_use]
    pub const fn member(&self) -> &ResolvedMember {
        &self.member
    }

    /// The member's own authoritative configuration document.
    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    /// The absolute root discovery runs against for this member
    /// (`member_root` joined with the member document's `[project].root`).
    #[must_use]
    pub const fn discover_root(&self) -> &AbsPath {
        &self.discover_root
    }

    /// The resolved change baseline ref: the `[[members]]` `base_ref` when set,
    /// else the member document's own `[project].base_ref`.
    #[must_use]
    pub fn base_ref(&self) -> Option<&str> {
        self.base_ref.as_deref()
    }
}

/// The fully composed federation: every member's authoritative config plus the
/// umbrella-level cross-member overlays and groups layered on top.
#[derive(Debug, Clone)]
pub struct ComposedFederation {
    members: Vec<ComposedMember>,
    overlays: Vec<OverlayConfig>,
    groups: BTreeMap<String, GroupConfig>,
}

impl ComposedFederation {
    /// The composed members, in declaration order.
    #[must_use]
    pub fn members(&self) -> &[ComposedMember] {
        &self.members
    }

    /// The umbrella-level cross-member overlay edges (empty in the degenerate
    /// single-repo case, where overlays are member-local on the lone document).
    #[must_use]
    pub fn overlays(&self) -> &[OverlayConfig] {
        &self.overlays
    }

    /// The umbrella-level cross-member groups (empty in the degenerate case).
    #[must_use]
    pub const fn groups(&self) -> &BTreeMap<String, GroupConfig> {
        &self.groups
    }
}

/// Compose the umbrella `document` and its enumerated `members`.
///
/// In the degenerate single-repo case (one member with no id) the lone member's
/// authoritative document **is** the umbrella document, and its overlays/groups
/// stay member-local — the umbrella contributes no cross-member layer. In the
/// umbrella case each declared member's own `toven.toml` is loaded through the
/// strict engine loader, and the umbrella document's overlays/groups become the
/// cross-member layer.
///
/// `loaded` is the set of ecosystem ids with a compiled-in adapter and
/// `canonical` the known-ecosystem registry, both forwarded to the member loader.
///
/// # Errors
/// Returns a typed error when a declared member has no readable `toven.toml`, or
/// when a member document fails strict load/validation/dispatch.
pub fn compose_members(
    document: &Document,
    members: &[ResolvedMember],
    loaded: &BTreeSet<EcosystemId>,
    canonical: &CanonicalRegistry,
) -> AppResult<ComposedFederation> {
    let mut composed = Vec::with_capacity(members.len());
    for member in members {
        composed.push(compose_one(document, member, loaded, canonical)?);
    }

    // Only a real umbrella (members carry an id) layers a cross-member overlay /
    // group set; the degenerate member keeps its overlays/groups member-local.
    let is_umbrella = members.iter().any(|member| member.id().is_some());
    let (overlays, groups) = if is_umbrella {
        (document.overlays.clone(), document.groups.clone())
    } else {
        (Vec::new(), BTreeMap::new())
    };

    Ok(ComposedFederation {
        members: composed,
        overlays,
        groups,
    })
}

/// Compose a single member: resolve its authoritative document, discovery root,
/// and effective baseline.
fn compose_one(
    umbrella: &Document,
    member: &ResolvedMember,
    loaded: &BTreeSet<EcosystemId>,
    canonical: &CanonicalRegistry,
) -> AppResult<ComposedMember> {
    let document = if member.id().is_some() {
        load_member_document(member, loaded, canonical)?
    } else {
        umbrella.clone()
    };

    let discover_root = resolve_discover_root(member, &document)?;
    let base_ref = member
        .base_ref()
        .or(document.project.base_ref.as_deref())
        .map(str::to_owned);

    Ok(ComposedMember {
        member: member.clone(),
        document,
        discover_root,
        base_ref,
    })
}

/// Load a declared member's own `toven.toml` through the strict engine loader.
fn load_member_document(
    member: &ResolvedMember,
    loaded: &BTreeSet<EcosystemId>,
    canonical: &CanonicalRegistry,
) -> AppResult<Document> {
    let config_path = member.root().as_path().join(MEMBER_CONFIG_FILENAME);
    if !config_path.is_file() {
        return Err(AppError::not_found(
            "members.toven",
            Some(&format!(
                "declared member '{}' has no {MEMBER_CONFIG_FILENAME} at '{}'; every member must be a runnable toven project",
                member.name(),
                config_path.display()
            )),
        ));
    }
    Ok(load(&config_path, loaded, canonical)?.document)
}

/// Resolve the absolute discovery root for a member: its repo root joined with
/// the member document's `[project].root`, confined at the trust boundary.
fn resolve_discover_root(member: &ResolvedMember, document: &Document) -> AppResult<AbsPath> {
    let relative = document.project.root();
    // `.` (the default project root) denotes the member repo root itself; the
    // safe-path validator rejects it as a non-normal segment, so short-circuit.
    if relative == "." {
        return Ok(member.root().clone());
    }
    validate_safe_path(relative).map_err(|error| {
        AppError::invalid_input(
            "project.root",
            format!(
                "member '{}' project root '{relative}' is not a safe relative path",
                member.name()
            ),
        )
        .with_cause(error)
    })?;
    let joined = safe_join(member.root().as_path(), relative).map_err(|error| {
        AppError::invalid_input(
            "project.root",
            format!(
                "member '{}' project root '{relative}' escapes the member root",
                member.name()
            ),
        )
        .with_cause(error)
    })?;
    AbsPath::new(joined)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use toven_model::AbsPath;

    use super::compose_members;
    use crate::config::{CanonicalRegistry, Document, MemberConfig, ProjectConfig, TovenConfig};
    use crate::federation::members::enumerate_members;

    fn umbrella(members: Vec<MemberConfig>) -> Document {
        Document {
            project: ProjectConfig {
                name: "umbrella".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: BTreeMap::new(),
            members,
        }
    }

    fn member(name: &str, root: &str, base_ref: Option<&str>) -> MemberConfig {
        MemberConfig {
            name: name.to_string(),
            root: root.to_string(),
            base_ref: base_ref.map(str::to_owned),
        }
    }

    #[test]
    fn degenerate_member_reuses_the_umbrella_document() {
        let ws = toven_testkit::workspace::workspace("compose-degenerate");
        let root = AbsPath::new(ws.path().to_path_buf()).unwrap();
        let mut document = umbrella(Vec::new());
        document.project.base_ref = Some("origin/main".to_string());
        let members = enumerate_members(&document, &root).unwrap();

        let composed = compose_members(
            &document,
            &members,
            &BTreeSet::new(),
            &CanonicalRegistry::model(),
        )
        .unwrap();

        assert_eq!(composed.members().len(), 1);
        let only = &composed.members()[0];
        assert_eq!(only.document().project.name, "umbrella");
        assert_eq!(only.discover_root(), &root);
        assert_eq!(only.base_ref(), Some("origin/main"));
        assert!(composed.overlays().is_empty());
        assert!(composed.groups().is_empty());
    }

    #[test]
    fn umbrella_loads_member_documents_and_layers_cross_member_overlays() {
        let ws = toven_testkit::workspace::workspace("compose-umbrella");
        ws.write_file(
            "repos/core/toven.toml",
            b"[project]\nname = \"core\"\nbase_ref = \"main\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
        )
        .unwrap();
        ws.write_file(
            "repos/services/toven.toml",
            b"[project]\nname = \"services\"\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n",
        )
        .unwrap();
        let root = AbsPath::new(ws.path().to_path_buf()).unwrap();

        let mut document = umbrella(vec![
            member("core", "repos/core", None),
            member("services", "repos/services", Some("release")),
        ]);
        document.overlays.push(crate::config::OverlayConfig {
            from: crate::config::OverlayRef {
                ecosystem: toven_model::EcosystemId::new("rust").unwrap(),
                module: "gateway".to_string(),
            },
            to: crate::config::OverlayRef {
                ecosystem: toven_model::EcosystemId::new("rust").unwrap(),
                module: "core".to_string(),
            },
        });
        let members = enumerate_members(&document, &root).unwrap();

        let composed = compose_members(
            &document,
            &members,
            &BTreeSet::new(),
            &CanonicalRegistry::model(),
        )
        .unwrap();

        assert_eq!(composed.members().len(), 2);
        assert_eq!(composed.members()[0].document().project.name, "core");
        // base_ref falls back to the member document's own [project].base_ref.
        assert_eq!(composed.members()[0].base_ref(), Some("main"));
        // an explicit [[members]].base_ref overrides the member document default.
        assert_eq!(composed.members()[1].base_ref(), Some("release"));
        assert_eq!(composed.overlays().len(), 1);
    }

    #[test]
    fn declared_member_without_toven_toml_is_an_error() {
        let ws = toven_testkit::workspace::workspace("compose-missing");
        ws.write_file("repos/core/.keep", b"").unwrap();
        let root = AbsPath::new(ws.path().to_path_buf()).unwrap();
        let document = umbrella(vec![member("core", "repos/core", None)]);
        let members = enumerate_members(&document, &root).unwrap();

        let error = compose_members(
            &document,
            &members,
            &BTreeSet::new(),
            &CanonicalRegistry::model(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("toven project"));
    }
}
