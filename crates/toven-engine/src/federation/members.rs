//! Member enumeration for the cross-repo umbrella.
//!
//! Turns the umbrella `[[members]]` array (or the degenerate single-`[project]`
//! case) into the resolved, root-confined member list the rest of the cross-repo
//! federation composes, discovers, and releases against.
//!
//! Enumeration resolves each declared member's repo root under the umbrella root
//! at the trust boundary (`validate_safe_path` + `safe_join`, the same
//! confinement the include loader uses), and treats an absent declared member as
//! a hard error rather than a warn-and-skip — a declared member is a required
//! graph node.

use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_validation::input::validate_safe_path;
use toven_model::{AbsPath, MemberId};

use crate::config::Document;

/// One resolved umbrella member: its identity, repo root, and change baseline.
///
/// `id` is `None` for the degenerate single-repo case (a lone `[project]` with no
/// `[[members]]`); modules under such a member are left unstamped so the
/// single-repo path stays byte-for-byte identical. `id` is `Some` for every
/// declared `[[members]]` entry, and those modules are stamped with it during
/// discovery so the model `member` slot becomes load-bearing.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedMember {
    id: Option<MemberId>,
    name: String,
    root: AbsPath,
    base_ref: Option<String>,
}

impl ResolvedMember {
    /// The member identity, or `None` for the degenerate single-repo member.
    #[must_use]
    pub const fn id(&self) -> Option<&MemberId> {
        self.id.as_ref()
    }

    /// The human-facing member name (`[project].name` or `[[members]].name`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The absolute member repo root (the directory holding its `toven.toml`).
    #[must_use]
    pub const fn root(&self) -> &AbsPath {
        &self.root
    }

    /// The per-member change baseline ref, if one was configured.
    #[must_use]
    pub fn base_ref(&self) -> Option<&str> {
        self.base_ref.as_deref()
    }
}

/// Resolve the umbrella `[[members]]` array into root-confined members.
///
/// `umbrella_root` is the absolute directory the umbrella `toven.toml` resolves
/// against (its project root). An empty `[[members]]` array yields exactly one
/// implicit, unstamped member rooted there — the degenerate single-repo case is
/// the same code path with N = 1, not a separate legacy branch. Each declared
/// member's `root` is confined under `umbrella_root` at the trust boundary and
/// must exist on disk.
///
/// # Errors
/// Returns a typed error when a declared member `root` escapes the umbrella root,
/// is absent on disk (with a hint to provision or clone it), or when two members
/// resolve to the same repo root.
pub fn enumerate_members(
    document: &Document,
    umbrella_root: &AbsPath,
) -> AppResult<Vec<ResolvedMember>> {
    if document.members.is_empty() {
        return Ok(vec![ResolvedMember {
            id: None,
            name: document.project.name.clone(),
            root: umbrella_root.clone(),
            base_ref: document.project.base_ref.clone(),
        }]);
    }

    let mut members = Vec::with_capacity(document.members.len());
    let mut seen_roots: BTreeSet<AbsPath> = BTreeSet::new();
    for member in &document.members {
        let root = resolve_member_root(umbrella_root, &member.name, &member.root)?;
        if !seen_roots.insert(root.clone()) {
            return Err(AppError::invalid_input(
                "members.root",
                format!(
                    "members resolve to the same repo root '{root}'; each member must be a distinct repo"
                ),
            ));
        }
        members.push(ResolvedMember {
            id: Some(MemberId::new(member.name.clone())?),
            name: member.name.clone(),
            root,
            base_ref: member.base_ref.clone(),
        });
    }
    Ok(members)
}

/// Confine one declared member `root` under the umbrella root and require it to
/// exist as a directory on disk.
fn resolve_member_root(umbrella_root: &AbsPath, name: &str, relative: &str) -> AppResult<AbsPath> {
    if relative == "." {
        return Err(AppError::invalid_input(
            "members.root",
            format!(
                "member '{name}' root '.' points at the umbrella root; omit [[members]] for a single-repo workspace or choose a child repo path"
            ),
        ));
    }
    validate_safe_path(relative).map_err(|error| {
        AppError::invalid_input(
            "members.root",
            format!("member '{name}' root '{relative}' is not a safe relative path"),
        )
        .with_cause(error)
    })?;
    let joined = safe_join(umbrella_root.as_path(), relative).map_err(|error| {
        AppError::invalid_input(
            "members.root",
            format!("member '{name}' root '{relative}' escapes the umbrella root"),
        )
        .with_cause(error)
    })?;
    if !joined.is_dir() {
        return Err(AppError::not_found(
            "members.root",
            Some(&format!(
                "declared member '{name}' is missing at '{}'; provision or clone it at the configured path",
                joined.display()
            )),
        ));
    }
    AbsPath::new(joined)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::AbsPath;

    use super::enumerate_members;
    use crate::config::{Document, MemberConfig, ProjectConfig, TovenConfig};

    fn document(members: Vec<MemberConfig>) -> Document {
        Document {
            project: ProjectConfig {
                name: "umbrella".to_string(),
                root: ".".to_string(),
                base_ref: Some("origin/main".to_string()),
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: BTreeMap::new(),
            members,
        }
    }

    fn member(name: &str, root: &str) -> MemberConfig {
        MemberConfig {
            name: name.to_string(),
            root: root.to_string(),
            base_ref: None,
        }
    }

    fn umbrella_root(ws: &toven_testkit::TestWorkspace) -> AbsPath {
        AbsPath::new(ws.path().to_path_buf()).unwrap()
    }

    #[test]
    fn empty_members_yields_one_unstamped_degenerate_member() {
        let ws = toven_testkit::workspace::workspace("members-degenerate");
        let root = umbrella_root(&ws);

        let members = enumerate_members(&document(Vec::new()), &root).unwrap();

        assert_eq!(members.len(), 1);
        assert!(members[0].id().is_none());
        assert_eq!(members[0].name(), "umbrella");
        assert_eq!(members[0].root(), &root);
        assert_eq!(members[0].base_ref(), Some("origin/main"));
    }

    #[test]
    fn declared_members_are_confined_and_stamped() {
        let ws = toven_testkit::workspace::workspace("members-declared");
        ws.write_file("repos/core/toven.toml", b"").unwrap();
        ws.write_file("repos/services/toven.toml", b"").unwrap();
        let root = umbrella_root(&ws);
        let doc = document(vec![
            member("core", "repos/core"),
            member("services", "repos/services"),
        ]);

        let members = enumerate_members(&doc, &root).unwrap();

        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id().unwrap().as_str(), "core");
        assert!(members[0].root().as_path().ends_with("repos/core"));
        assert_eq!(members[1].id().unwrap().as_str(), "services");
    }

    #[test]
    fn absent_declared_member_is_a_hard_error() {
        let ws = toven_testkit::workspace::workspace("members-absent");
        let root = umbrella_root(&ws);
        let doc = document(vec![member("core", "repos/core")]);

        let error = enumerate_members(&doc, &root).unwrap_err();
        assert!(error.to_string().contains("provision or clone"));
    }

    #[test]
    fn member_root_escaping_umbrella_is_rejected() {
        let ws = toven_testkit::workspace::workspace("members-escape");
        let root = umbrella_root(&ws);
        let doc = document(vec![member("escape", "../outside")]);

        assert!(enumerate_members(&doc, &root).is_err());
    }

    #[test]
    fn duplicate_member_roots_are_rejected() {
        let ws = toven_testkit::workspace::workspace("members-duplicate-root");
        ws.write_file("repos/core/toven.toml", b"").unwrap();
        let root = umbrella_root(&ws);
        let doc = document(vec![
            member("core", "repos/core"),
            member("again", "repos/core"),
        ]);

        let error = enumerate_members(&doc, &root).unwrap_err();

        assert!(
            error.to_string().contains("same repo root"),
            "error should explain duplicate member roots: {error}"
        );
    }

    #[test]
    fn dot_member_root_gets_an_actionable_error() {
        let ws = toven_testkit::workspace::workspace("members-dot-root");
        let root = umbrella_root(&ws);
        let doc = document(vec![member("umbrella", ".")]);

        let error = enumerate_members(&doc, &root).unwrap_err();

        assert!(
            error.to_string().contains("omit [[members]]")
                && error.to_string().contains("child repo path"),
            "error should explain how to model the workspace: {error}"
        );
    }
}
