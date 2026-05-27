//! Project configuration loading and normalization.

mod normalize;
mod raw;

pub use normalize::{load_workspace, normalize_config};
pub use raw::{RawConfig, RawProfile, RawTask, RawWorkspace};
