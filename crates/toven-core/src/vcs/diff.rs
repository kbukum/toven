//! The change foundation: resolve a [`DiffRange`] of two endpoints onto the
//! [`VcsReader`] seam.
//!
//! This is the reusable answer to "what changed between two points" every verb
//! shares. It resolves each [`DiffEndpoint`] to a concrete ref (or the working
//! tree), then maps the `{from, to}` pair onto the git seam:
//! - a working-tree target composes committed `from..HEAD` ∪ working-tree
//!   status (mirroring affected-selection's committed-∪-worktree union);
//! - two committed endpoints diff directly via
//!   [`changed_between`](VcsReader::changed_between).
//!
//! Baseline *policy* (merge-base, config precedence) stays in the engine's
//! `BaselineStrategy`; this module only resolves endpoints and diffs.

use rskit_errors::{AppError, AppResult};
use toven_ports::{ChangeRecord, DiffEndpoint, DiffRange, VcsReader};

use super::tags::latest_matching;

/// An endpoint resolved to something the git seam can diff.
enum Resolved {
    /// The uncommitted working tree.
    WorkingTree,
    /// A concrete committed ref (branch, tag, oid, or `HEAD`).
    Committed(String),
}

/// Resolve every endpoint of `range` and diff them, treating a `LatestTag`
/// endpoint that matches no tag as an error.
///
/// # Errors
/// Returns an error when a required `LatestTag` endpoint matches no tag, when
/// the working tree is used as a *baseline* (only a target is meaningful), or
/// when the underlying reader fails.
pub fn resolve_range(reader: &dyn VcsReader, range: &DiffRange) -> AppResult<Vec<ChangeRecord>> {
    resolve_range_optional(reader, range)?.ok_or_else(|| {
        AppError::invalid_input(
            "vcs.range",
            "no tag matched the requested latest-tag endpoint",
        )
    })
}

/// Resolve every endpoint of `range` and diff them, returning `None` when a
/// `LatestTag` endpoint matches no tag so the caller owns the "never tagged"
/// policy.
///
/// # Errors
/// Returns an error when the working tree is used as a *baseline* (only a
/// target is meaningful) or when the underlying reader fails.
pub fn resolve_range_optional(
    reader: &dyn VcsReader,
    range: &DiffRange,
) -> AppResult<Option<Vec<ChangeRecord>>> {
    let (Some(from), Some(to)) = (
        resolve_endpoint(reader, &range.from)?,
        resolve_endpoint(reader, &range.to)?,
    ) else {
        return Ok(None);
    };

    let records = match (from, to) {
        (from, Resolved::WorkingTree) => {
            let base = committed_ref(from, "baseline")?;
            let mut changed = reader.changed_between(&base, "HEAD")?;
            changed.extend(reader.worktree_status()?);
            changed
        }
        (Resolved::WorkingTree, _) => {
            return Err(AppError::invalid_input(
                "vcs.range",
                "the working tree can only be a comparison target, not a baseline",
            ));
        }
        (Resolved::Committed(from), Resolved::Committed(to)) => {
            reader.changed_between(&from, &to)?
        }
    };
    Ok(Some(records))
}

/// Resolve a single endpoint to a diffable ref, or `None` when a `LatestTag`
/// endpoint matches no tag.
fn resolve_endpoint(
    reader: &dyn VcsReader,
    endpoint: &DiffEndpoint,
) -> AppResult<Option<Resolved>> {
    let resolved = match endpoint {
        DiffEndpoint::WorkingTree => Resolved::WorkingTree,
        DiffEndpoint::Head => Resolved::Committed("HEAD".to_string()),
        DiffEndpoint::Ref(name) => Resolved::Committed(name.clone()),
        DiffEndpoint::Oid(oid) => Resolved::Committed(oid.as_str().to_string()),
        DiffEndpoint::LatestTag { scheme } => {
            let tags = reader.list_tags(None)?;
            match latest_matching(scheme, &tags) {
                Some((_, tag)) => Resolved::Committed(tag.name),
                None => return Ok(None),
            }
        }
        _ => {
            return Err(AppError::invalid_input(
                "vcs.range",
                "unsupported diff endpoint",
            ));
        }
    };
    Ok(Some(resolved))
}

/// Require a committed endpoint (a working tree cannot be a baseline `role`).
fn committed_ref(resolved: Resolved, role: &str) -> AppResult<String> {
    match resolved {
        Resolved::Committed(reference) => Ok(reference),
        Resolved::WorkingTree => Err(AppError::invalid_input(
            "vcs.range",
            format!("the working tree cannot be used as the {role} of a comparison"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use toven_ports::{ChangeRecord, DiffEndpoint, DiffRange, TagScheme};
    use toven_testkit::TestWorkspace;
    use toven_testkit::git::GitScenario;

    use super::{resolve_range, resolve_range_optional};
    use crate::vcs::RskitGitVcs;

    /// A scripted scenario: one commit per named file, a feature branch, and a
    /// release tag, so every enumerated comparison has a distinct answer.
    ///
    /// History:
    /// - `main`: `base.txt` (v1.0.0 tag) → `on_main.txt`
    /// - `feature` (off the tag): `on_feature.txt`
    /// - working tree: an uncommitted `dirty.txt`
    struct Fixture {
        _workspace: TestWorkspace,
        vcs: RskitGitVcs,
        tag_commit: String,
        head: String,
        feature: String,
    }

    impl Fixture {
        fn build() -> Self {
            let workspace = TestWorkspace::new("vcs-diff-endpoints");
            let scenario = GitScenario::init(workspace.path()).expect("git init");
            scenario
                .commit_file("base.txt", "base", "base")
                .expect("base commit");
            scenario.tag_lightweight("rust/app@1.0.0").expect("tag");
            let tag_commit = scenario.resolve("HEAD").expect("tag commit");

            scenario.branch("feature").expect("branch feature");

            scenario
                .commit_file("on_main.txt", "main", "on main")
                .expect("main commit");
            let head = scenario.resolve("HEAD").expect("head");

            scenario.checkout("feature").expect("checkout feature");
            scenario
                .commit_file("on_feature.txt", "feature", "on feature")
                .expect("feature commit");
            let feature = scenario.resolve("HEAD").expect("feature head");

            scenario.checkout("main").expect("checkout main");
            scenario
                .write_file("dirty.txt", "dirty")
                .expect("dirty file");

            let vcs = RskitGitVcs::open(workspace.path()).expect("open");
            Self {
                _workspace: workspace,
                vcs,
                tag_commit,
                head,
                feature,
            }
        }

        fn paths(records: &[ChangeRecord]) -> BTreeSet<PathBuf> {
            records.iter().map(|record| record.path.clone()).collect()
        }
    }

    fn paths(names: &[&str]) -> BTreeSet<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn scheme() -> TagScheme {
        TagScheme::new("rust/app@", "")
    }

    #[test]
    fn resolves_every_enumerated_comparison() {
        let fx = Fixture::build();

        // (from, to, expected changed paths) for each enumerated scenario.
        let cases: Vec<(DiffEndpoint, DiffEndpoint, BTreeSet<PathBuf>)> = vec![
            // commit↔commit: tag commit → main HEAD adds on_main.txt.
            (
                DiffEndpoint::reference(fx.tag_commit.clone()),
                DiffEndpoint::reference(fx.head.clone()),
                paths(&["on_main.txt"]),
            ),
            // branch↔branch: main → feature diverges (main dropped, feature added).
            (
                DiffEndpoint::reference("main"),
                DiffEndpoint::reference("feature"),
                paths(&["on_main.txt", "on_feature.txt"]),
            ),
            // commit↔tag: main HEAD → tag commit removes on_main.txt.
            (
                DiffEndpoint::reference(fx.head.clone()),
                DiffEndpoint::reference(fx.tag_commit.clone()),
                paths(&["on_main.txt"]),
            ),
            // branch↔tag: feature branch → the tag ref by name.
            (
                DiffEndpoint::reference("feature"),
                DiffEndpoint::reference("rust/app@1.0.0"),
                paths(&["on_feature.txt"]),
            ),
            // working-tree↔branch: tag → working tree (on_main.txt committed +
            // dirty.txt uncommitted).
            (
                DiffEndpoint::reference("rust/app@1.0.0"),
                DiffEndpoint::WorkingTree,
                paths(&["on_main.txt", "dirty.txt"]),
            ),
            // working-tree↔commit: main HEAD → working tree is just the dirty file.
            (
                DiffEndpoint::reference(fx.head.clone()),
                DiffEndpoint::WorkingTree,
                paths(&["dirty.txt"]),
            ),
            // current↔latest-tag: latest matching tag → working tree.
            (
                DiffEndpoint::latest_tag(scheme()),
                DiffEndpoint::WorkingTree,
                paths(&["on_main.txt", "dirty.txt"]),
            ),
        ];

        for (from, to, expected) in cases {
            let range = DiffRange::new(from.clone(), to.clone());
            let records = resolve_range(&fx.vcs, &range)
                .unwrap_or_else(|error| panic!("resolve {from:?}→{to:?}: {error}"));
            assert_eq!(
                Fixture::paths(&records),
                expected,
                "unexpected changes for {from:?} → {to:?}",
            );
        }

        // Ensure the feature endpoint id was actually distinct from HEAD.
        assert_ne!(fx.feature, fx.head);
    }

    #[test]
    fn optional_returns_none_when_no_tag_matches() {
        let fx = Fixture::build();
        let range = DiffRange::new(
            DiffEndpoint::latest_tag(TagScheme::new("go/app@", "")),
            DiffEndpoint::WorkingTree,
        );

        let resolved =
            resolve_range_optional(&fx.vcs, &range).expect("optional resolve does not error");

        assert!(resolved.is_none(), "no go/app@ tag exists");
    }

    #[test]
    fn required_latest_tag_without_a_match_is_a_typed_error() {
        let fx = Fixture::build();
        let range = DiffRange::new(
            DiffEndpoint::latest_tag(TagScheme::new("go/app@", "")),
            DiffEndpoint::WorkingTree,
        );

        let error =
            resolve_range(&fx.vcs, &range).expect_err("required latest tag with no match errors");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
    }

    #[test]
    fn working_tree_baseline_is_rejected() {
        let fx = Fixture::build();
        let range = DiffRange::new(DiffEndpoint::WorkingTree, DiffEndpoint::Head);

        let error = resolve_range(&fx.vcs, &range).expect_err("working-tree baseline is rejected");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
    }
}
