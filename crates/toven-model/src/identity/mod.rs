//! Identity + topology vocabulary: the references everyone speaks.

mod ecosystem;
mod member_id;
mod module_key;
mod module_ref;
mod paths;
mod workspace_id;

pub use ecosystem::EcosystemId;
pub use member_id::MemberId;
pub use module_key::ModuleKey;
pub use module_ref::ModuleRef;
pub use paths::{AbsPath, RepoPath};
pub use workspace_id::WorkspaceId;
