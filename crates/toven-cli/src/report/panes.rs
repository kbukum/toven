//! [`PaneRawSink`] — the opt-in multiplexer adapter for the engine's raw-output
//! channel.
//!
//! Under a supported multiplexer (tmux, detected via `$TMUX`) this sink gives
//! each of the first [`PANE_CAP`](super::view::PANE_CAP) in-flight units its own
//! real pane with independent scrollback and selection, and renders any overflow
//! units — plus every unit's fallback path — through an embedded
//! [`TilesRawSink`]. It is the richest live view but only sensible for a handful
//! of long-lived units (`--watch`, a few heavy crates), which is why it stays
//! opt-in and capped.
//!
//! The multiplexer mechanics live behind the [`PaneLauncher`] port so the
//! routing/cap policy is testable without spawning tmux; [`TmuxLauncher`] is the
//! argv-only production launcher. If a pane cannot be opened the unit degrades to
//! the embedded tiles renderer, so output is never lost.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

use rskit_cli::Palette;
use rskit_errors::{AppError, AppResult};
use rskit_fs::sync_io::file as fs_file;
use rskit_process::{ProcessConfig, ProcessSpec, PtySize};
use toven_model::{UnitOutput, UnitStatus};
use toven_ports::RawOutputSink;

use super::tiles::{TilesRawSink, verdict_line};

/// Opens and drives one multiplexer pane per unit.
///
/// A pure port so the cap/overflow policy in [`PaneRawSink`] is unit-testable
/// with a fake; the production implementation is [`TmuxLauncher`].
pub trait PaneLauncher: Send {
    /// Open a pane for unit `id` labeled `label`, returning its live handle.
    ///
    /// # Errors
    /// Propagates any launch failure; the caller degrades that unit to tiles.
    fn open(&mut self, id: &str, label: &str) -> AppResult<Box<dyn PaneHandle>>;
}

/// A live handle to one open pane.
pub trait PaneHandle: Send {
    /// Append raw child bytes to the pane.
    ///
    /// # Errors
    /// Propagates any write failure.
    fn write(&mut self, bytes: &[u8]) -> AppResult<()>;

    /// Clear the pane and re-label it for a fresh rerun, so a reused pane shows
    /// only the current run rather than accumulating every watch iteration.
    ///
    /// # Errors
    /// Propagates any write failure.
    fn reset(&mut self, label: &str) -> AppResult<()>;

    /// Append the final `verdict` line to the pane; the pane stays open so its
    /// scrollback survives the run.
    ///
    /// # Errors
    /// Propagates any write failure.
    fn finish(&mut self, verdict: &str) -> AppResult<()>;
}

/// A tmux-backed `PaneLauncher`: each unit gets a pane tailing a private temp
/// file the sink appends to.
pub struct TmuxLauncher {
    dir: PathBuf,
    size: PtySize,
    seq: usize,
}

impl TmuxLauncher {
    /// Create a launcher whose per-unit temp files live under `dir`, opening
    /// panes sized to `size`.
    #[must_use]
    pub const fn new(dir: PathBuf, size: PtySize) -> Self {
        Self { dir, size, seq: 0 }
    }
}

impl PaneLauncher for TmuxLauncher {
    fn open(&mut self, id: &str, label: &str) -> AppResult<Box<dyn PaneHandle>> {
        let path = self.dir.join(format!("pane-{}.log", self.seq));
        self.seq += 1;
        let mut file = fs_file::create(&path)?;
        writeln!(file, "• {label}").map_err(AppError::internal)?;
        file.flush().map_err(AppError::internal)?;
        let result = rskit_process::run(
            &ProcessSpec::new("tmux").args(split_window_args(&path, self.size)),
            &ProcessConfig::default(),
        )?;
        if !result.success() {
            return Err(AppError::internal(std::io::Error::other(format!(
                "tmux split-window failed for unit `{id}`"
            ))));
        }
        Ok(Box::new(TmuxPane { file, path }))
    }
}

/// The argv for `tmux split-window` that tails `path` in a new pane sized to
/// `size`. Argv-only (no shell); `-d` keeps focus on the current pane.
fn split_window_args(path: &std::path::Path, size: PtySize) -> Vec<String> {
    vec![
        "split-window".to_string(),
        "-d".to_string(),
        "-l".to_string(),
        size.rows.max(1).to_string(),
        "--".to_string(),
        "tail".to_string(),
        "-n".to_string(),
        "+1".to_string(),
        "-f".to_string(),
        path.to_string_lossy().into_owned(),
    ]
}

/// One open tmux pane: appends land in the tailed temp file.
struct TmuxPane {
    file: File,
    path: PathBuf,
}

impl PaneHandle for TmuxPane {
    fn write(&mut self, bytes: &[u8]) -> AppResult<()> {
        self.file.write_all(bytes).map_err(AppError::internal)?;
        self.file.flush().map_err(AppError::internal)
    }

    fn reset(&mut self, label: &str) -> AppResult<()> {
        self.file.set_len(0).map_err(AppError::internal)?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(AppError::internal)?;
        writeln!(self.file, "• {label}").map_err(AppError::internal)?;
        self.file.flush().map_err(AppError::internal)
    }

    fn finish(&mut self, verdict: &str) -> AppResult<()> {
        writeln!(self.file, "{verdict}").map_err(AppError::internal)?;
        self.file.flush().map_err(AppError::internal)
    }
}

impl Drop for TmuxPane {
    fn drop(&mut self) {
        // Best-effort: the pane keeps the tailed content in its own scrollback,
        // so removing the backing temp file on teardown leaks nothing visible.
        let _ = fs_file::remove_if_exists(&self.path);
    }
}

/// Renders raw output as multiplexer panes (up to a cap), tiling the overflow.
pub struct PaneRawSink {
    launcher: Box<dyn PaneLauncher>,
    palette: Palette,
    cap: usize,
    panes: HashMap<String, Box<dyn PaneHandle>>,
    tiles: TilesRawSink,
}

impl PaneRawSink {
    /// Create a pane sink driving `launcher` for up to `cap` concurrent panes,
    /// tiling the overflow through `tiles`, and coloring verdicts per `palette`.
    #[must_use]
    pub fn new(
        launcher: Box<dyn PaneLauncher>,
        cap: usize,
        palette: Palette,
        tiles: TilesRawSink,
    ) -> Self {
        Self {
            launcher,
            palette,
            cap,
            panes: HashMap::new(),
            tiles,
        }
    }
}

impl RawOutputSink for PaneRawSink {
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()> {
        if let Some(pane) = self.panes.get_mut(&chunk.unit_id) {
            return pane.write(&chunk.bytes);
        }
        self.tiles.live(chunk)
    }

    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()> {
        self.tiles.block(unit_id, chunks)
    }

    fn supports_concurrent_live(&self) -> bool {
        true
    }

    fn begin_unit(&mut self, unit_id: &str, label: &str) -> AppResult<()> {
        // Reuse a unit's existing pane across watch reruns instead of opening a
        // fresh one each iteration (which would leak panes for the session).
        if let Some(pane) = self.panes.get_mut(unit_id) {
            return pane.reset(label);
        }
        if self.panes.len() < self.cap {
            match self.launcher.open(unit_id, label) {
                Ok(pane) => {
                    self.panes.insert(unit_id.to_string(), pane);
                    return Ok(());
                }
                // A pane that will not open degrades to a tile so the unit still
                // streams live rather than losing its output.
                Err(_) => return self.tiles.begin_unit(unit_id, label),
            }
        }
        self.tiles.begin_unit(unit_id, label)
    }

    fn end_unit(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        // Keep the pane open (and mapped) so a subsequent watch rerun reuses it;
        // the pane's temp file is reclaimed when the sink is dropped.
        if let Some(pane) = self.panes.get_mut(unit_id) {
            return pane.finish(&verdict_line(self.palette, unit_id, status));
        }
        self.tiles.end_unit(unit_id, status)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rskit_cli::Palette;
    use rskit_errors::AppResult;
    use rskit_process::PtySize;
    use toven_model::{OutputStream, UnitOutput, UnitStatus};
    use toven_ports::RawOutputSink;

    use super::{PaneHandle, PaneLauncher, PaneRawSink, split_window_args};
    use crate::report::tiles::TilesRawSink;

    #[derive(Default)]
    struct Recorded {
        opened: Vec<String>,
        writes: Vec<(String, Vec<u8>)>,
        resets: Vec<String>,
        finished: Vec<(String, String)>,
    }

    #[derive(Clone, Default)]
    struct FakeLauncher {
        rec: Arc<Mutex<Recorded>>,
        fail: bool,
    }

    struct FakePane {
        id: String,
        rec: Arc<Mutex<Recorded>>,
    }

    impl PaneLauncher for FakeLauncher {
        fn open(&mut self, id: &str, _label: &str) -> AppResult<Box<dyn PaneHandle>> {
            if self.fail {
                return Err(rskit_errors::AppError::internal(std::io::Error::other(
                    "no tmux",
                )));
            }
            self.rec.lock().unwrap().opened.push(id.to_string());
            Ok(Box::new(FakePane {
                id: id.to_string(),
                rec: self.rec.clone(),
            }))
        }
    }

    impl PaneHandle for FakePane {
        fn write(&mut self, bytes: &[u8]) -> AppResult<()> {
            self.rec
                .lock()
                .unwrap()
                .writes
                .push((self.id.clone(), bytes.to_vec()));
            Ok(())
        }

        fn reset(&mut self, _label: &str) -> AppResult<()> {
            self.rec.lock().unwrap().resets.push(self.id.clone());
            Ok(())
        }

        fn finish(&mut self, verdict: &str) -> AppResult<()> {
            self.rec
                .lock()
                .unwrap()
                .finished
                .push((self.id.clone(), verdict.to_string()));
            Ok(())
        }
    }

    fn chunk(unit: &str, bytes: &[u8]) -> UnitOutput {
        UnitOutput {
            unit_id: unit.into(),
            stream: OutputStream::Stdout,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn split_window_args_are_argv_only_and_tail_the_file() {
        let args = split_window_args(
            std::path::Path::new("/tmp/pane-0.log"),
            PtySize::new(10, 80),
        );
        assert_eq!(args[0], "split-window");
        assert!(args.contains(&"--".to_string()));
        assert_eq!(args.last().unwrap(), "/tmp/pane-0.log");
        assert!(args.contains(&"tail".to_string()));
    }

    #[test]
    fn routes_within_cap_to_panes_and_overflow_to_tiles() {
        let launcher = FakeLauncher::default();
        let rec = launcher.rec.clone();
        let mut sink = PaneRawSink::new(
            Box::new(launcher),
            2,
            Palette::new(false),
            TilesRawSink::hidden(),
        );

        sink.begin_unit("a", "a").unwrap();
        sink.begin_unit("b", "b").unwrap();
        sink.begin_unit("c", "c").unwrap(); // over cap → tiles

        sink.live(&chunk("a", b"a-out\n")).unwrap();
        sink.live(&chunk("c", b"c-out\n")).unwrap(); // to tiles, not a pane

        sink.end_unit("a", UnitStatus::Succeeded).unwrap();
        sink.end_unit("c", UnitStatus::Failed).unwrap();

        let rec = rec.lock().unwrap();
        assert_eq!(rec.opened, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(rec.writes, vec![("a".to_string(), b"a-out\n".to_vec())]);
        assert_eq!(rec.finished.len(), 1);
        assert_eq!(rec.finished[0].0, "a");
        drop(rec);
    }

    #[test]
    fn a_reused_unit_resets_its_pane_instead_of_reopening() {
        // Across watch reruns the same unit must reuse its pane (reset), never
        // open a second one — otherwise panes leak for the whole session.
        let launcher = FakeLauncher::default();
        let rec = launcher.rec.clone();
        let mut sink = PaneRawSink::new(
            Box::new(launcher),
            4,
            Palette::new(false),
            TilesRawSink::hidden(),
        );

        sink.begin_unit("a", "a").unwrap();
        sink.end_unit("a", UnitStatus::Succeeded).unwrap();
        // Second rerun of the same unit.
        sink.begin_unit("a", "a").unwrap();
        sink.end_unit("a", UnitStatus::Failed).unwrap();

        let (opened, resets, finished_len) = {
            let rec = rec.lock().unwrap();
            (rec.opened.clone(), rec.resets.clone(), rec.finished.len())
        };
        assert_eq!(opened, vec!["a".to_string()], "pane opened once");
        assert_eq!(resets, vec!["a".to_string()], "reused via reset");
        assert_eq!(finished_len, 2, "each rerun writes its verdict");
    }

    #[test]
    fn a_failed_pane_open_degrades_that_unit_to_tiles() {
        let launcher = FakeLauncher {
            fail: true,
            ..FakeLauncher::default()
        };
        let mut sink = PaneRawSink::new(
            Box::new(launcher),
            4,
            Palette::new(false),
            TilesRawSink::hidden(),
        );
        // Must not error: the unit falls back to the embedded tiles renderer.
        sink.begin_unit("a", "a").unwrap();
        sink.live(&chunk("a", b"still streamed\n")).unwrap();
        sink.end_unit("a", UnitStatus::Succeeded).unwrap();
    }
}
