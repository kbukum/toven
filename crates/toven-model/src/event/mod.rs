//! Observability vocabulary: the closed [`Event`] stream the engine emits.
//!
//! The engine emits typed `Event`s only; it never formats. A `Reporter` sink
//! (defined with the ports) renders them. Because the stream is typed and
//! serializable it also crosses the out-of-process driver boundary as
//! diagnostics. Raw child output is deliberately *not* per-line vocabulary — it
//! flows through the separate, coarse [`UnitOutput`] channel so high-throughput
//! build output never pays per-line (de)serialization.

mod coverage;
mod outcome;
mod output;
mod phase;
mod record;
mod stats;
mod status;

pub use coverage::{CoverageMeasurement, CoverageMetric, CoverageVerdict};
pub use outcome::OutcomeSummary;
pub use output::{OutputStream, UnitOutput};
pub use phase::Phase;
pub use record::Event;
pub use stats::RunStats;
pub use status::UnitStatus;
