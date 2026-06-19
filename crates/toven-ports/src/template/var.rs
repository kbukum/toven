//! `TaskVar` — Toven's typed placeholder namespace over rskit-util's
//! [`Placeholder`](rskit_util::Placeholder) trait.

use std::fmt;

use rskit_util::Placeholder;

/// The closed set of argv template variables.
///
/// The *vocabulary* is Toven's; parsing, strict unknown-placeholder rejection,
/// and substitution are rskit-util's [`Template`](rskit_util::Template).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum TaskVar {
    /// `{args}` — user passthrough tail, spliced verbatim (a list-valued splice
    /// point, not a single value; never re-templated).
    Args,
    /// `{project.root}` — repo root.
    ProjectRoot,
    /// `{workspace.root}` — first-class workspace root.
    WorkspaceRoot,
    /// `{module.name}` — `Module.id.name`.
    ModuleName,
    /// `{module.package}` — package/crate name (falls back to name).
    ModulePackage,
    /// `{module.root}` — repo-relative module root.
    ModuleRoot,
    /// `{module.manifest}` — manifest path.
    ModuleManifest,
    /// `{module.selector}` — explicit per-module selector splice point.
    ModuleSelector,
}

impl TaskVar {
    /// Every placeholder, for [`Template::parse`](rskit_util::Template::parse).
    pub const ALL: &'static [Self] = &[
        Self::Args,
        Self::ProjectRoot,
        Self::WorkspaceRoot,
        Self::ModuleName,
        Self::ModulePackage,
        Self::ModuleRoot,
        Self::ModuleManifest,
        Self::ModuleSelector,
    ];
}

impl Placeholder for TaskVar {
    fn token(self) -> &'static str {
        match self {
            Self::Args => "args",
            Self::ProjectRoot => "project.root",
            Self::WorkspaceRoot => "workspace.root",
            Self::ModuleName => "module.name",
            Self::ModulePackage => "module.package",
            Self::ModuleRoot => "module.root",
            Self::ModuleManifest => "module.manifest",
            Self::ModuleSelector => "module.selector",
        }
    }
}

impl fmt::Display for TaskVar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}
