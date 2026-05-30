//! CLI composition root.

mod affected;
mod app;
mod cache;
mod commands;
mod dispatch;
mod explain;
mod graph;
mod modules;
mod plan;
mod run;
mod watch;

pub use app::{command, run};
