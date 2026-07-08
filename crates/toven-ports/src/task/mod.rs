//! Tasks vocabulary — the kind, command shape, and scheduling attributes an
//! adapter produces and the engine schedules, caches, and executes.

mod fan_out;
mod intent;
mod kind;
mod origin;
mod probe;
mod readiness;
mod spec;

pub use fan_out::FanOut;
pub use intent::TaskIntent;
pub use kind::TaskKind;
pub use origin::TaskOrigin;
pub use probe::ToolchainProbe;
pub use readiness::{DEFAULT_READINESS_TIMEOUT, Readiness};
pub use spec::Task;
