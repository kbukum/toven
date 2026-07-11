//! Resolve the effective live-output rendering for a run.
//!
//! The [`ViewMode`] preference (from `--view` or `[toven].view`) is folded with
//! the environment — is stderr a real terminal, is a multiplexer present, how
//! many units will run — into a concrete [`ResolvedView`]. The engine's live
//! streaming is Unix-and-PTY-first, so any non-terminal target always collapses
//! to [`ResolvedView::Stream`] (the byte-for-byte log-friendly fallback),
//! keeping piped, redirected, `--output jsonl`, and CI runs unchanged.

use rskit_process::PtySize;
use toven_engine::config::ViewMode;

use super::tiles::TILE_TAIL_LINES;

/// Most units to render as real multiplexer panes before the rest fall back to
/// tiles; beyond a handful, panes are unusable.
pub(super) const PANE_CAP: usize = 6;

/// The concrete rendering chosen for a run, plus the PTY size live units get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedView {
    /// Single linear stream with no live area. Normal-unit output that could
    /// interleave under parallelism is buffered into a deterministic per-unit
    /// block; live-safe runs (serial/single-unit, no held persistent unit) still
    /// stream inline. On a terminal live units attach a PTY so child colors are
    /// preserved; a redirected or piped target keeps the byte-stable pipe shape.
    Stream,
    /// Live in-terminal tiles; each unit's PTY is sized to `pty`.
    Tiles {
        /// PTY size each live unit is allocated (tile tail height × terminal width).
        pty: PtySize,
    },
    /// One multiplexer pane per unit up to `cap`, the rest as tiles; each unit's
    /// PTY is sized to `pty`.
    Panes {
        /// Most units rendered as real panes before the rest fall back to tiles.
        cap: usize,
        /// PTY size each live unit is allocated (matches the terminal).
        pty: PtySize,
    },
}

/// Fold a [`ViewMode`] preference with the environment into a [`ResolvedView`].
///
/// `terminal` is the stderr terminal size when stderr is a real terminal (else
/// `None`); `in_multiplexer` is whether a supported multiplexer (tmux) is
/// active; `units` is the planned unit count.
#[must_use]
pub fn resolve_view(
    view: ViewMode,
    terminal: Option<PtySize>,
    in_multiplexer: bool,
    units: usize,
) -> ResolvedView {
    // A non-terminal target cannot host a live area or a PTY, so every mode —
    // including an explicit `tiles`/`panes` — collapses to the stream fallback.
    let Some(term) = terminal else {
        return ResolvedView::Stream;
    };
    let tile_pty = PtySize::new(TILE_TAIL_LINES, term.cols.max(1));
    match view {
        // Panes need a multiplexer host; inside one render real panes, otherwise
        // (like plain tiles) render tiles rather than spawning a doomed
        // `tmux split-window` once per unit.
        ViewMode::Panes if in_multiplexer => ResolvedView::Panes {
            cap: PANE_CAP,
            pty: term,
        },
        ViewMode::Tiles | ViewMode::Panes => ResolvedView::Tiles { pty: tile_pty },
        ViewMode::Auto => {
            if in_multiplexer && units <= PANE_CAP {
                ResolvedView::Panes {
                    cap: PANE_CAP,
                    pty: term,
                }
            } else {
                ResolvedView::Tiles { pty: tile_pty }
            }
        }
        // `ViewMode::Stream`, plus any future `#[non_exhaustive]` mode, uses the
        // deterministic stream renderer.
        _ => ResolvedView::Stream,
    }
}

#[cfg(test)]
mod tests {
    use super::{PANE_CAP, ResolvedView, ViewMode, resolve_view};
    use rskit_process::PtySize;

    const TERM: PtySize = PtySize::new(40, 120);

    #[test]
    fn non_terminal_always_streams_even_when_tiles_requested() {
        assert_eq!(
            resolve_view(ViewMode::Tiles, None, false, 3),
            ResolvedView::Stream
        );
        assert_eq!(
            resolve_view(ViewMode::Panes, None, true, 2),
            ResolvedView::Stream
        );
        assert_eq!(
            resolve_view(ViewMode::Auto, None, true, 1),
            ResolvedView::Stream
        );
    }

    #[test]
    fn explicit_stream_streams_on_a_terminal() {
        assert_eq!(
            resolve_view(ViewMode::Stream, Some(TERM), true, 3),
            ResolvedView::Stream
        );
    }

    #[test]
    fn explicit_tiles_sizes_pty_to_terminal_width() {
        match resolve_view(ViewMode::Tiles, Some(TERM), false, 12) {
            ResolvedView::Tiles { pty } => {
                assert_eq!(pty.cols, TERM.cols);
                assert_eq!(pty.rows, super::TILE_TAIL_LINES);
            }
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn auto_picks_panes_in_a_multiplexer_within_cap() {
        assert_eq!(
            resolve_view(ViewMode::Auto, Some(TERM), true, PANE_CAP),
            ResolvedView::Panes {
                cap: PANE_CAP,
                pty: TERM
            }
        );
    }

    #[test]
    fn auto_falls_back_to_tiles_when_units_exceed_pane_cap() {
        match resolve_view(ViewMode::Auto, Some(TERM), true, PANE_CAP + 1) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn auto_uses_tiles_on_a_plain_terminal_without_a_multiplexer() {
        match resolve_view(ViewMode::Auto, Some(TERM), false, 2) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn explicit_panes_outside_a_multiplexer_falls_back_to_tiles() {
        // `--view panes` needs a multiplexer host; without one it degrades to
        // tiles up front instead of spawning a doomed `tmux split-window` per unit.
        match resolve_view(ViewMode::Panes, Some(TERM), false, 3) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn explicit_panes_in_a_multiplexer_resolves_panes() {
        assert!(matches!(
            resolve_view(ViewMode::Panes, Some(TERM), true, 3),
            ResolvedView::Panes { .. }
        ));
    }
}
