//! Bind the resolved live-output view to a concrete raw-output sink and PTY.
//!
//! This is the one place that turns a [`ViewMode`] preference into the running
//! sink the engine writes through and the PTY sizing the process runner uses.
//! The live tiles/panes area needs a pseudoterminal, so it is Unix-first: on a
//! non-Unix target — or when the machine JSON projection is active, when stderr
//! is not a terminal, or when `--view stream` is chosen — this falls back to
//! the byte-stable [`WriterRawSink`] with live units still attached to a PTY
//! matching the terminal (a no-op when stderr is redirected), preserving
//! today's stream shape byte-for-byte.

use std::path::Path;

use rskit_cli::Palette;
use rskit_errors::AppResult;
use toven_engine::apply::ProcessCommandRunner;
use toven_engine::config::ViewMode;
use toven_ports::RawOutputSink;

use super::WriterRawSink;

/// Configure `runner` and build the raw-output sink for the resolved view.
///
/// Returns the (possibly PTY-enabled) runner and the sink the engine's
/// [`UnitOutputChannel`](toven_engine::output::UnitOutputChannel) writes
/// through. `force_stream` pins the log-friendly stream shape (set for the JSON
/// projection); `unit_count` seeds the `auto` tiles-vs-panes choice; `pane_dir`
/// is where the tmux launcher keeps its per-unit temp files.
///
/// # Errors
/// Propagates a failure to create the pane temp directory.
#[cfg(unix)]
pub(crate) fn configure_live_output(
    runner: ProcessCommandRunner,
    view: ViewMode,
    force_stream: bool,
    palette: Palette,
    unit_count: usize,
    max_parallel: usize,
    pane_dir: &Path,
) -> AppResult<(ProcessCommandRunner, Box<dyn RawOutputSink>)> {
    use super::view::{ResolvedView, resolve_view};
    use super::{PaneRawSink, TilesRawSink, TmuxLauncher};

    let resolved = if force_stream {
        ResolvedView::Stream
    } else {
        let terminal = rskit_process::terminal_size(&std::io::stderr());
        let in_multiplexer = std::env::var_os("TMUX").is_some();
        resolve_view(view, terminal, in_multiplexer, unit_count, max_parallel)
    };

    Ok(match resolved {
        // The stream fallback keeps today's shape. `force_stream` (the machine JSON projection)
        // must stay byte-stable, so it keeps deterministic pipe capture with no PTY; an interactive
        // `--view stream` still attaches a PTY matching the terminal (a no-op when stderr is
        // redirected) so child colors are preserved.
        ResolvedView::Stream => {
            let runner = if force_stream {
                runner
            } else {
                runner.with_pty_matching_terminal(&std::io::stderr())
            };
            (runner, Box::new(WriterRawSink::stderr()))
        }
        ResolvedView::Tiles { pty } => {
            let tiles = TilesRawSink::stderr(pty.cols as usize, palette);
            // Size the child to the tile's inner grid, not the full tile width: a child
            // told it is `pty.cols` wide wraps a full-width in-place progress redraw at the
            // narrower grid edge, scrolling the short grid and leaking a stale frame to
            // scrollback on every tick.
            let child = grid_pty(pty, tiles.content_cols());
            (runner.with_pty(child), Box::new(tiles))
        }
        ResolvedView::Panes { cap, pty } => {
            rskit_fs::sync_io::dir::create_all(pane_dir)?;
            let tiles = TilesRawSink::stderr(pty.cols as usize, palette);
            let launcher = Box::new(TmuxLauncher::new(pane_dir.to_path_buf(), pty));
            (
                runner.with_pty(pty),
                Box::new(PaneRawSink::new(launcher, cap, palette, tiles)),
            )
        }
    })
}

/// Narrow a full-width tile PTY to the tile's inner grid width, keeping its row
/// count. Used so a live child's own line wrapping matches the visible grid.
#[cfg(unix)]
fn grid_pty(tile: rskit_process::PtySize, content_cols: usize) -> rskit_process::PtySize {
    rskit_process::PtySize::new(tile.rows, u16::try_from(content_cols).unwrap_or(tile.cols))
}

/// Non-Unix fallback: PTY-backed live views are unavailable, so every run uses
/// the deterministic pipe-backed [`WriterRawSink`] (today's behavior).
#[cfg(not(unix))]
pub(crate) fn configure_live_output(
    runner: ProcessCommandRunner,
    view: ViewMode,
    force_stream: bool,
    palette: Palette,
    unit_count: usize,
    max_parallel: usize,
    pane_dir: &Path,
) -> AppResult<(ProcessCommandRunner, Box<dyn RawOutputSink>)> {
    let _ = (
        view,
        force_stream,
        palette,
        unit_count,
        max_parallel,
        pane_dir,
    );
    Ok((
        runner.with_pty_matching_terminal(&std::io::stderr()),
        Box::new(WriterRawSink::stderr()),
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;

    use rskit_cli::Palette;
    use toven_engine::apply::ProcessCommandRunner;
    use toven_engine::config::ViewMode;
    use toven_ports::RawOutputSink;

    use super::configure_live_output;
    use super::grid_pty;
    use rskit_process::PtySize;

    #[test]
    fn grid_pty_narrows_cols_to_the_content_width_keeping_rows() {
        // A live child is sized to the tile's inner grid, not the full tile width, so
        // its wrapping matches the grid and progress redraws don't scroll the short
        // grid into scrollback.
        let child = grid_pty(PtySize::new(6, 120), 118);
        assert_eq!(child.rows, 6);
        assert_eq!(child.cols, 118);
    }

    #[test]
    fn force_stream_pins_the_stream_sink_even_when_tiles_is_requested() {
        // The machine JSON projection sets `force_stream`, which must keep the
        // byte-stable single-stream shape (a non-concurrent-live sink) so a piped
        // consumer is byte-for-byte unaffected — even under `--view tiles`.
        let runner = ProcessCommandRunner::new(Path::new("."));
        let (_runner, sink) = configure_live_output(
            runner,
            ViewMode::Tiles,
            true,
            Palette::new(false),
            8,
            4,
            Path::new("/tmp/toven-live-test-panes"),
        )
        .expect("configures");
        assert!(!sink.supports_concurrent_live());
    }
}
