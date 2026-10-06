//! Go import path → repo-relative file path mapping.
//!
//! `go test -coverprofile` names each file by its import path
//! (`example.com/repo/svc/cmd/main.go`), not by its location on disk. The
//! aggregator attributes files by repo-relative path, so a Go profile is
//! translated first: a record whose import path lies under a discovered Go
//! module's path (the `module` directive in its `go.mod`, carried as
//! [`Module::package`]) is rewritten to that module's root plus the rest of the
//! path. The longest matching module path wins, so a nested module claims its
//! own files from an enclosing root module.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::Module;

/// The manifest file name whose `module` directive defines Go import paths.
const GO_MANIFEST: &str = "go.mod";

/// Discovered Go module paths and their repo-relative roots.
#[derive(Debug, Clone, Default)]
pub(super) struct GoImportRoots {
    /// `(module path, module root)`, longest module path first.
    roots: Vec<(String, PathBuf)>,
}

impl GoImportRoots {
    /// Collect every module declared by a `go.mod` that carries its module
    /// path. Other modules (other ecosystems) never contribute, so a crate name
    /// cannot shadow a Go import path.
    #[must_use]
    pub(super) fn from_modules(modules: &[Module]) -> Self {
        let mut roots: Vec<(String, PathBuf)> = modules
            .iter()
            .filter(|module| is_go_module(module))
            .filter_map(|module| {
                let path = module.package.as_deref()?.trim_end_matches('/');
                (!path.is_empty()).then(|| (path.to_string(), module.root.as_path().to_path_buf()))
            })
            .collect();
        roots.sort_by(|(left, left_root), (right, right_root)| {
            right
                .len()
                .cmp(&left.len())
                .then(left.cmp(right))
                .then(left_root.cmp(right_root))
        });
        roots.dedup();
        Self { roots }
    }

    /// The repo-relative file path for a Go import path, or `None` when no
    /// discovered module path is a whole-segment prefix of it.
    ///
    /// # Errors
    /// Returns [`ErrorCode::Conflict`] when the matching module path is declared
    /// at more than one root (for example by two federation members): a profile
    /// does not say which one it came from, so guessing would mix their numbers.
    pub(super) fn resolve(&self, import_path: &str) -> AppResult<Option<PathBuf>> {
        let mut matches = self.roots.iter().filter_map(|(module_path, root)| {
            let rest = import_path
                .strip_prefix(module_path.as_str())?
                .strip_prefix('/')
                .filter(|rest| !rest.is_empty())?;
            Some((module_path, root, rest))
        });
        let Some((module_path, root, rest)) = matches.next() else {
            return Ok(None);
        };
        let others: Vec<String> = matches
            .take_while(|(other, _, _)| *other == module_path)
            .map(|(_, other_root, _)| other_root.display().to_string())
            .collect();
        if !others.is_empty() {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!(
                    "Go module path '{module_path}' is declared at more than one root ({}, {}); \
                     coverage for '{import_path}' cannot be attributed",
                    root.display(),
                    others.join(", ")
                ),
            ));
        }
        Ok(Some(if root.as_path() == Path::new(".") {
            PathBuf::from(rest)
        } else {
            root.join(rest)
        }))
    }
}

/// Whether `module` is declared by a `go.mod` manifest.
pub(super) fn is_go_module(module: &Module) -> bool {
    module
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.as_path().file_name())
        .is_some_and(|name| name == GO_MANIFEST)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::GoImportRoots;
    use crate::coverage::test_support::{go_module as go, module};

    #[test]
    fn nested_module_maps_to_its_root() {
        let roots = GoImportRoots::from_modules(&[go(
            "go-services",
            "go-services",
            "example.com/runlab/go-services",
        )]);
        assert_eq!(
            roots
                .resolve("example.com/runlab/go-services/cmd/gateway/main.go")
                .expect("resolves"),
            Some(PathBuf::from("go-services/cmd/gateway/main.go"))
        );
    }

    #[test]
    fn longest_module_path_wins_over_the_root_module() {
        let roots = GoImportRoots::from_modules(&[
            go("runlab", ".", "example.com/runlab"),
            go("gen-go", "gen/go", "example.com/runlab/gen/go"),
        ]);
        assert_eq!(
            roots
                .resolve("example.com/runlab/gen/go/api/v1/api.pb.go")
                .expect("resolves"),
            Some(PathBuf::from("gen/go/api/v1/api.pb.go"))
        );
        assert_eq!(
            roots
                .resolve("example.com/runlab/tools/lint.go")
                .expect("resolves"),
            Some(PathBuf::from("tools/lint.go"))
        );
    }

    #[test]
    fn module_path_must_match_whole_segments() {
        let roots = GoImportRoots::from_modules(&[go("cache", "cache", "ex/cache")]);
        assert_eq!(roots.resolve("ex/cachex/a.go").expect("resolves"), None);
        assert_eq!(roots.resolve("ex/cache").expect("resolves"), None);
    }

    #[test]
    fn unknown_import_path_is_unresolved() {
        let roots = GoImportRoots::from_modules(&[go("svc", "svc", "example.com/svc")]);
        assert_eq!(
            roots.resolve("golang.org/x/text/a.go").expect("resolves"),
            None
        );
    }

    #[test]
    fn one_module_path_at_two_roots_is_ambiguous() {
        let roots = GoImportRoots::from_modules(&[
            go("left", "members/left", "example.com/lib"),
            go("right", "members/right", "example.com/lib"),
        ]);
        let error = roots
            .resolve("example.com/lib/a.go")
            .expect_err("two roots for one module path");
        let message = error.to_string();
        assert!(message.contains("example.com/lib"), "{message}");
        assert!(
            message.contains("members/left") && message.contains("members/right"),
            "{message}"
        );
        assert_eq!(
            roots.resolve("example.com/other/a.go").expect("unrelated"),
            None
        );
    }

    #[test]
    fn non_go_modules_never_contribute() {
        let mut crate_module = module("rust", "ex", "crates/ex", Some("crates/ex/Cargo.toml"));
        crate_module.package = Some("ex".to_string());
        let roots = GoImportRoots::from_modules(&[crate_module]);
        assert_eq!(roots.resolve("ex/lib.go").expect("resolves"), None);
    }
}
