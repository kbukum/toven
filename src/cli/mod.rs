//! CLI composition root.

mod affected;
mod app;
mod explain;
mod plan;
mod run;

pub use app::{command, run};
