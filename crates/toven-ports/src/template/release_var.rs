//! `ReleaseVar` — the typed placeholder namespace for release tag/commit
//! templates (`tag_format`, `tag_message`, `commit_message`).

use std::fmt;

use rskit_util::Placeholder;

/// The closed set of placeholders a release tag/commit template may reference.
///
/// The *vocabulary* is Toven's; parsing and strict unknown-placeholder rejection
/// are rskit-util's [`Template`](rskit_util::Template). A configured template is
/// validated against this set at config time so a typo like `{verison}` fails the
/// load rather than silently rendering an empty segment at tag time.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum ReleaseVar {
    /// `{version}` — the resolved release version (`1.2.3`, `1.0.0-rc.1`).
    Version,
    /// `{ecosystem}` — the releasing module's ecosystem id (`rust`).
    Ecosystem,
    /// `{module}` — the releasing module's name (`toven-core`).
    Module,
    /// `{channel}` — the prerelease channel, empty for a stable release.
    Channel,
}

impl ReleaseVar {
    /// Every placeholder, for [`Template::parse`](rskit_util::Template::parse).
    pub const ALL: &'static [Self] = &[Self::Version, Self::Ecosystem, Self::Module, Self::Channel];
}

impl Placeholder for ReleaseVar {
    fn token(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Ecosystem => "ecosystem",
            Self::Module => "module",
            Self::Channel => "channel",
        }
    }
}

impl fmt::Display for ReleaseVar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}
