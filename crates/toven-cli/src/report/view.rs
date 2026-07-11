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
/// active; `units` is the planned unit count, or `0` when it is unknown (watch
/// mode) or the plan is empty — in `auto` a `0` count never selects panes.
/// `max_parallel` is the effective concurrency ceiling for the run.
///
/// In `auto`, a run that cannot interleave — a serial ceiling (`max_parallel <=
/// 1`) or a single unit — resolves to [`ResolvedView::Stream`]: live tiles/panes
/// exist only to de-interleave concurrent output, so with one emitter at a time
/// the inline stream is the cleaner, log-friendly shape. An explicit
/// `tiles`/`panes`/`stream` is always honored regardless of concurrency.
#[must_use]
pub fn resolve_view(
    view: ViewMode,
    terminal: Option<PtySize>,
    in_multiplexer: bool,
    units: usize,
    max_parallel: usize,
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
        // A serial or single-unit run has no concurrent output to de-interleave,
        // so live tiles/panes add nothing; stream inline instead.
        ViewMode::Auto if max_parallel <= 1 || units == 1 => ResolvedView::Stream,
        ViewMode::Auto => {
            // `auto` picks panes only for a known small run: a positive unit
            // count within the cap. `units == 0` means the count is unknown
            // (watch mode) or the plan is empty, which is not a "small run", so
            // it uses tiles rather than defaulting to panes under tmux.
            if in_multiplexer && (1..=PANE_CAP).contains(&units) {
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
    /// A parallel ceiling large enough that `auto` never collapses to the serial
    /// stream, so a test exercises the terminal renderer it names.
    const PARALLEL: usize = 8;

    #[test]
    fn non_terminal_always_streams_even_when_tiles_requested() {
        assert_eq!(
            resolve_view(ViewMode::Tiles, None, false, 3, PARALLEL),
            ResolvedView::Stream
        );
        assert_eq!(
            resolve_view(ViewMode::Panes, None, true, 2, PARALLEL),
            ResolvedView::Stream
        );
        assert_eq!(
            resolve_view(ViewMode::Auto, None, true, 1, PARALLEL),
            ResolvedView::Stream
        );
    }

    #[test]
    fn explicit_stream_streams_on_a_terminal() {
        assert_eq!(
            resolve_view(ViewMode::Stream, Some(TERM), true, 3, PARALLEL),
            ResolvedView::Stream
        );
    }

    #[test]
    fn auto_streams_a_serial_run_on_a_terminal() {
        // `--jobs 1` (or `max_parallel = 1`): nothing can interleave, so `auto`
        // uses the inline stream rather than a live tile per unit.
        assert_eq!(
            resolve_view(ViewMode::Auto, Some(TERM), true, PANE_CAP, 1),
            ResolvedView::Stream
        );
    }

    #[test]
    fn auto_streams_a_single_unit_run_even_when_parallel() {
        // One unit cannot interleave with anything, so a live area is pointless.
        assert_eq!(
            resolve_view(ViewMode::Auto, Some(TERM), false, 1, PARALLEL),
            ResolvedView::Stream
        );
    }

    #[test]
    fn explicit_tiles_are_honored_on_a_serial_run() {
        // An explicit `--view tiles` is argv-sacred: honored even serially.
        match resolve_view(ViewMode::Tiles, Some(TERM), false, 3, 1) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn explicit_tiles_sizes_pty_to_terminal_width() {
        match resolve_view(ViewMode::Tiles, Some(TERM), false, 12, PARALLEL) {
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
            resolve_view(ViewMode::Auto, Some(TERM), true, PANE_CAP, PARALLEL),
            ResolvedView::Panes {
                cap: PANE_CAP,
                pty: TERM
            }
        );
    }

    #[test]
    fn auto_uses_tiles_when_unit_count_is_unknown_or_empty_under_a_multiplexer() {
        // `units == 0` (watch mode's unknown count, or an empty plan) must not
        // satisfy the small-run pane rule; it resolves to tiles under tmux.
        match resolve_view(ViewMode::Auto, Some(TERM), true, 0, PARALLEL) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles for a 0 unit count, got {other:?}"),
        }
    }

    #[test]
    fn auto_falls_back_to_tiles_when_units_exceed_pane_cap() {
        match resolve_view(ViewMode::Auto, Some(TERM), true, PANE_CAP + 1, PARALLEL) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn auto_uses_tiles_on_a_plain_terminal_without_a_multiplexer() {
        match resolve_view(ViewMode::Auto, Some(TERM), false, 2, PARALLEL) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn explicit_panes_outside_a_multiplexer_falls_back_to_tiles() {
        // `--view panes` needs a multiplexer host; without one it degrades to
        // tiles up front instead of spawning a doomed `tmux split-window` per unit.
        match resolve_view(ViewMode::Panes, Some(TERM), false, 3, PARALLEL) {
            ResolvedView::Tiles { .. } => {}
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn explicit_panes_in_a_multiplexer_resolves_panes() {
        assert!(matches!(
            resolve_view(ViewMode::Panes, Some(TERM), true, 3, PARALLEL),
            ResolvedView::Panes { .. }
        ));
    }
}
