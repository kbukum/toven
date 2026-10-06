//! Shared test fixtures for the coverage modules.

use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};

/// A module in `ecosystem` rooted at `root`, with an optional manifest.
pub(super) fn module(ecosystem: &str, name: &str, root: &str, manifest: Option<&str>) -> Module {
    let mut module = Module::new(
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap(),
        RepoPath::new(root).unwrap(),
    );
    module.manifest = manifest.map(|manifest| RepoPath::new(manifest).unwrap());
    module
}

/// A Go module at `root` whose `go.mod` declares module path `package`.
pub(super) fn go_module(name: &str, root: &str, package: &str) -> Module {
    let manifest = if root == "." {
        "go.mod".to_string()
    } else {
        format!("{root}/go.mod")
    };
    let mut module = module("go", name, root, Some(&manifest));
    module.package = Some(package.to_string());
    module
}
