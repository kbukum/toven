//! Stable verb identity for project-level lifecycle hooks.
//!
//! [`VerbId`] names a toven command that can carry `pre`/`post` lifecycle hooks
//! from `toven.toml` (`[hooks.<verb>]`). It is both the strict config map key —
//! `#[serde(deny_unknown_fields)]` on the [`Document`](super::Document) plus this
//! closed enum reject any unknown verb key at parse time — and the label the
//! driver uses when it composes and runs a verb's hooks.
//!
//! The release family (`bump`/`tag`/`publish`) is addressable distinctly from
//! the umbrella `release` key; [`Document::hooks_for`](super::Document::hooks_for)
//! defines their precedence (both apply, the specific verb innermost).

use serde::{Deserialize, Serialize};

/// A toven verb that can carry project-level lifecycle hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum VerbId {
    /// `toven run <task>` (and the bare argv-first task form).
    Run,
    /// `toven plan <task>`.
    Plan,
    /// `toven coverage`.
    Coverage,
    /// `toven doctor`.
    Doctor,
    /// `toven release` — the umbrella key whose hooks apply to every release
    /// mutation (`bump`/`tag`/`publish`), outermost around the specific verb.
    Release,
    /// `toven release bump`.
    Bump,
    /// `toven release tag`.
    Tag,
    /// `toven release publish`.
    Publish,
}

impl VerbId {
    /// The verb's canonical name (its config key and diagnostic label).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Plan => "plan",
            Self::Coverage => "coverage",
            Self::Doctor => "doctor",
            Self::Release => "release",
            Self::Bump => "bump",
            Self::Tag => "tag",
            Self::Publish => "publish",
        }
    }
}
