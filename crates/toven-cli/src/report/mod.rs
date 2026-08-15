//! Observability sinks and exit mapping.
//!
//! The engine emits a closed, typed [`Event`](toven_model::Event) stream; these
//! sinks render it. Two built-ins ship — [`HumanReporter`] (terminal
//! tables/progress lines) and [`JsonlReporter`] (machine-parseable Event
//! stream) — and slot in future ones (GH-annotations, `JUnit`) with no engine
//! change. The per-unit raw child-output channel is rendered by one of three
//! views — [`WriterRawSink`] (linear `stream`), [`TilesRawSink`] (live
//! in-terminal tiles), or the tmux-backed `PaneRawSink` — selected by
//! `configure_live_output`. [`exit_code`] derives the process exit from a run
//! summary.

mod exit;
mod human;
mod jsonl;
mod live;
mod output;
#[cfg(unix)]
mod panes;
mod summary;
mod tiles;
#[cfg(unix)]
mod view;

pub use exit::{exit_code, terminal_exit_code};
pub use human::HumanReporter;
pub use jsonl::JsonlReporter;
pub(crate) use live::configure_live_output;
pub use output::WriterRawSink;
#[cfg(unix)]
pub use panes::{PaneRawSink, TmuxLauncher};
pub use tiles::TilesRawSink;
#[cfg(unix)]
pub use view::{ResolvedView, resolve_view};
