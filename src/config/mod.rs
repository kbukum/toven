//! Project configuration loading and normalization.

mod document;
mod normalize;

pub use document::{ConfigDocument, ProfileConfig, TaskConfig, WorkspaceConfig};
pub use normalize::{load_workspace, normalize_config};
