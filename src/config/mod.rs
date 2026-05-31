//! Project configuration loading and normalization.

mod document;
mod load;
mod profile;
mod project;
pub mod scope;
mod task;
mod workspace;

pub use document::ConfigDocument;
pub use load::{load_workspace, normalize_config};
pub use profile::ProfileConfig;
pub use project::ProjectConfig;
pub use task::TaskConfig;
pub use workspace::WorkspaceConfig;
