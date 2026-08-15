//! Shared human-output theme resolution.

use rskit_cli::{Palette, Theme};

use crate::flags::ColorWhen;

/// Resolve the canonical human theme against process stderr.
#[must_use]
pub(crate) fn stderr_theme(color: ColorWhen) -> Theme {
    Theme::new(Palette::for_stream(color.into(), &std::io::stderr()))
}
