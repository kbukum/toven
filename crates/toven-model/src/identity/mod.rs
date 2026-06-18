//! Identity + topology vocabulary: the references everyone speaks.

mod ecosystem;
mod ids;
mod module_ref;
mod paths;

pub use ecosystem::EcosystemId;
pub use ids::{MemberId, WorkspaceId};
pub use module_ref::ModuleRef;
pub use paths::{AbsPath, RepoPath};
