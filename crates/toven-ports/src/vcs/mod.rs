//! The VCS port — the single git seam, split into a read side (everywhere) and
//! a history-mutating write side (release APPLY only).

mod baseline;
mod change;
mod reader;
mod reference;
mod writer;

pub use baseline::{BaselineMode, BaselineSpec};
pub use change::{ChangeRecord, ChangeStatus};
pub use reader::VcsReader;
pub use reference::{Oid, TagRef};
pub use writer::VcsWriter;
