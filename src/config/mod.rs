//! Project configuration loading and normalization.

mod cache;
mod dependency;
mod document;
mod load;
mod profile;
mod project;
pub mod scope;
mod task;

pub use cache::CacheConfig;
pub use dependency::DependencyOverlayConfig;
pub use document::ConfigDocument;
pub use load::{load_workspace, normalize_config};
pub use profile::ProfileConfig;
pub use project::ProjectConfig;
pub use scope::ScopeConfig;
pub use task::TaskConfig;
