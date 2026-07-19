//! APPLY spine: wave-driven exec, failure gating, cache recording, and
//! teardown.

mod entry;
mod exec;
mod gating;
mod options;
mod persistent;
mod pool;
mod record;
mod walk;

pub use entry::apply;
pub use exec::ProcessCommandRunner;
pub use options::ApplyOptions;
#[cfg(unix)]
pub use rskit_process::PtySize;
