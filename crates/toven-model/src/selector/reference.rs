//! The user-facing module selector grammar (parsed shape, pre-resolution).

use std::fmt;
use std::str::FromStr;

use rskit_errors::{AppError, AppResult};

use crate::identity::{EcosystemId, WorkspaceId};

use super::pattern::NamePattern;

/// A user-named module selector, parsed but not yet resolved against a graph.
///
/// Canonical identity stays `ecosystem:name`; this type only relaxes what is
/// *accepted* as input at the selection boundary. It carries the user's intent as
/// a pattern — a bare name/glob, an ecosystem-qualified name/glob, a
/// workspace-qualified name/glob, or a whole workspace — which the engine (the
/// layer that owns a graph) resolves into concrete module keys, surfacing an
/// ambiguous bare name as a typed error rather than guessing.
///
/// # Grammar
/// [`parse`](ModuleSelector::parse) applies one rule to a `--module` token:
/// - a `/` splits on the **rightmost** `/` into `workspace/name` (workspace ids
///   may themselves contain `:`, e.g. `rust:contrib/api`);
/// - otherwise a `:` splits on the **first** `:` into `ecosystem:name`;
/// - otherwise the whole token is a bare name.
///
/// The name segment (right of the split, or the whole bare token) is a
/// [`NamePattern`] — an exact name or a `*`/`?` glob.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum ModuleSelector {
    /// Bare name or glob, unqualified: resolve across the whole graph.
    Name(NamePattern),
    /// Ecosystem-qualified name or glob (`rust:core`, `rust:*`).
    Ecosystem {
        /// The ecosystem the name is scoped to.
        ecosystem: EcosystemId,
        /// The module-name pattern within that ecosystem.
        name: NamePattern,
    },
    /// Workspace-qualified name or glob (`backend/api`, `backend/*`).
    Workspace {
        /// The workspace the name is scoped to.
        workspace: WorkspaceId,
        /// The module-name pattern within that workspace.
        name: NamePattern,
    },
    /// A whole workspace by id pattern: every module the matching workspace owns.
    WholeWorkspace(NamePattern),
}

impl ModuleSelector {
    /// Parse a `--module`/verb selector token per the grammar above.
    ///
    /// # Errors
    /// Returns [`AppError::invalid_input`] when a qualifier or name segment is
    /// empty (`:core`, `rust:`, `/api`, `backend/`) or the qualifier is not a
    /// valid ecosystem/workspace id.
    pub fn parse(token: &str) -> AppResult<Self> {
        if let Some((workspace, name)) = token.rsplit_once('/') {
            let workspace = WorkspaceId::new(non_empty(workspace, "workspace")?)?;
            return Ok(Self::Workspace {
                workspace,
                name: name_pattern(name)?,
            });
        }
        if let Some((ecosystem, name)) = token.split_once(':') {
            let ecosystem = EcosystemId::new(non_empty(ecosystem, "ecosystem")?)?;
            return Ok(Self::Ecosystem {
                ecosystem,
                name: name_pattern(name)?,
            });
        }
        Ok(Self::Name(name_pattern(token)?))
    }

    /// Build a whole-workspace selector from a `--workspace` id-pattern token.
    ///
    /// The token is treated as a single workspace-id pattern (exact or `*`/`?`
    /// glob), never split on `:`/`/`, so `rust:contrib` addresses one workspace
    /// and `backend*` addresses several.
    ///
    /// # Errors
    /// Returns [`AppError::invalid_input`] when the token is empty.
    pub fn whole_workspace(token: &str) -> AppResult<Self> {
        Ok(Self::WholeWorkspace(NamePattern::parse(non_empty(
            token,
            "workspace",
        )?)))
    }
}

/// The `NamePattern` for a name segment, rejecting an empty segment.
fn name_pattern(segment: &str) -> AppResult<NamePattern> {
    Ok(NamePattern::parse(non_empty(segment, "module")?))
}

/// Return `value` when non-empty, else a typed empty-segment error for `field`.
fn non_empty<'a>(value: &'a str, field: &str) -> AppResult<&'a str> {
    if value.is_empty() {
        Err(AppError::invalid_input(
            field,
            format!("empty {field} segment in selector"),
        ))
    } else {
        Ok(value)
    }
}

impl FromStr for ModuleSelector {
    type Err = AppError;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        Self::parse(token)
    }
}

impl fmt::Display for ModuleSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(formatter, "{name}"),
            Self::Ecosystem { ecosystem, name } => write!(formatter, "{ecosystem}:{name}"),
            Self::Workspace { workspace, name } => write!(formatter, "{workspace}/{name}"),
            Self::WholeWorkspace(workspace) => write!(formatter, "{workspace}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ModuleSelector, NamePattern};

    fn name(pattern: &str) -> NamePattern {
        NamePattern::parse(pattern)
    }

    #[test]
    fn bare_name_is_unqualified() {
        assert_eq!(
            ModuleSelector::parse("api").unwrap(),
            ModuleSelector::Name(name("api"))
        );
    }

    #[test]
    fn bare_glob_is_unqualified() {
        assert_eq!(
            ModuleSelector::parse("rskit-*").unwrap(),
            ModuleSelector::Name(name("rskit-*"))
        );
    }

    #[test]
    fn colon_qualifies_the_ecosystem() {
        let ModuleSelector::Ecosystem { ecosystem, name } =
            ModuleSelector::parse("rust:core").unwrap()
        else {
            panic!("expected ecosystem-qualified");
        };
        assert_eq!(ecosystem.as_str(), "rust");
        assert_eq!(name, NamePattern::parse("core"));
    }

    #[test]
    fn ecosystem_glob_scopes_to_the_ecosystem() {
        let ModuleSelector::Ecosystem { ecosystem, name } =
            ModuleSelector::parse("rust:*").unwrap()
        else {
            panic!("expected ecosystem-qualified");
        };
        assert_eq!(ecosystem.as_str(), "rust");
        assert!(!name.is_exact());
    }

    #[test]
    fn slash_qualifies_the_workspace() {
        let ModuleSelector::Workspace { workspace, name } =
            ModuleSelector::parse("backend/api").unwrap()
        else {
            panic!("expected workspace-qualified");
        };
        assert_eq!(workspace.as_str(), "backend");
        assert_eq!(name, NamePattern::parse("api"));
    }

    #[test]
    fn rightmost_slash_splits_so_workspace_id_may_contain_a_colon() {
        let ModuleSelector::Workspace { workspace, name } =
            ModuleSelector::parse("rust:contrib/api").unwrap()
        else {
            panic!("expected workspace-qualified");
        };
        assert_eq!(workspace.as_str(), "rust:contrib");
        assert_eq!(name, NamePattern::parse("api"));
    }

    #[test]
    fn workspace_glob_scopes_to_the_workspace() {
        let ModuleSelector::Workspace { workspace, name } =
            ModuleSelector::parse("backend/*").unwrap()
        else {
            panic!("expected workspace-qualified");
        };
        assert_eq!(workspace.as_str(), "backend");
        assert!(!name.is_exact());
    }

    #[test]
    fn whole_workspace_is_a_single_id_pattern() {
        assert_eq!(
            ModuleSelector::whole_workspace("rust:contrib").unwrap(),
            ModuleSelector::WholeWorkspace(name("rust:contrib"))
        );
        let ModuleSelector::WholeWorkspace(pattern) =
            ModuleSelector::whole_workspace("backend*").unwrap()
        else {
            panic!("expected whole-workspace");
        };
        assert!(!pattern.is_exact());
    }

    #[test]
    fn empty_segments_are_rejected() {
        assert!(ModuleSelector::parse(":core").is_err());
        assert!(ModuleSelector::parse("rust:").is_err());
        assert!(ModuleSelector::parse("/api").is_err());
        assert!(ModuleSelector::parse("backend/").is_err());
        assert!(ModuleSelector::whole_workspace("").is_err());
    }

    #[test]
    fn empty_workspace_token_is_attributed_to_the_workspace_field() {
        let error = ModuleSelector::whole_workspace("").unwrap_err();
        assert!(error.to_string().contains("workspace"), "{error}");
    }

    #[test]
    fn from_str_parses_the_module_form() {
        assert_eq!(
            "rust:core".parse::<ModuleSelector>().unwrap(),
            ModuleSelector::parse("rust:core").unwrap()
        );
    }
}
