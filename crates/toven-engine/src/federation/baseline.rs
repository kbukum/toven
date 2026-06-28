//! Per-member change baselines and VCS readers for umbrella planning.
//!
//! Each composed member can resolve its own baseline ref, while a caller-provided
//! baseline flag applies the same ref name independently to every member repo.
//! The reader set keeps the existing rskit-git-backed per-repo dedup: callers own
//! the opened set and borrow a lightweight member-indexed view into PLAN.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{AbsPath, MemberId, WorkspaceId};
use toven_ports::{BaselineSpec, ChangeRecord, VcsReader};

use crate::federation::compose::{ComposedFederation, ComposedMember};
use crate::federation::rebase::member_prefix;
use crate::federation::release::{MemberReleaseRepo, MemberReleaseRepos};
use crate::vcs::{BaselineFlags, BaselineStrategy, VcsReaderSet, rebase_records};

/// Resolved baseline specs keyed by member.
#[derive(Debug, Clone)]
pub struct MemberBaselines {
    entries: Vec<MemberBaseline>,
}

impl MemberBaselines {
    /// Return the baseline for `member`, if one was resolved.
    #[must_use]
    pub fn get(&self, member: Option<&MemberId>) -> Option<&BaselineSpec> {
        self.entries
            .iter()
            .find(|entry| entry.member.as_ref() == member)
            .and_then(|entry| entry.spec.as_ref())
    }

    fn push(&mut self, member: Option<MemberId>, spec: Option<BaselineSpec>) {
        self.entries.push(MemberBaseline { member, spec });
    }
}

/// One member's resolved baseline spec, absent when neither the member config nor
/// the caller's flags name a reference.
#[derive(Debug, Clone)]
struct MemberBaseline {
    member: Option<MemberId>,
    spec: Option<BaselineSpec>,
}

/// Resolve every member's effective change baseline.
///
/// `flags.base` overrides each member's configured ref with the same ref name,
/// while `flags.merge_base` only changes the mode. Without a flag ref, each
/// member uses its effective composed `base_ref`; a member with neither resolves
/// to `None` (a changed-selection over it then errors at consumption time).
#[must_use]
pub fn resolve_baselines(composed: &ComposedFederation, flags: &BaselineFlags) -> MemberBaselines {
    let mut baselines = MemberBaselines {
        entries: Vec::with_capacity(composed.members().len()),
    };
    for member in composed.members() {
        let spec = BaselineStrategy::resolve_optional(flags, member.base_ref());
        baselines.push(member.member().id().cloned(), spec);
    }
    baselines
}

/// Opened per-member readers backed by the shared per-repo reader set.
#[derive(Debug)]
pub struct OpenMemberVcsReaders {
    set: VcsReaderSet,
    entries: Vec<OpenMemberVcsReader>,
}

impl OpenMemberVcsReaders {
    /// Borrow this opened set as the reader view consumed by PLAN.
    #[must_use]
    pub fn readers(&self) -> MemberVcsReaders<'_> {
        let entries = self
            .entries
            .iter()
            .map(|entry| MemberVcsReader {
                member: entry.member.clone(),
                prefix: entry.prefix.clone(),
                repo_prefix: entry.repo_prefix.clone(),
                baseline: entry.baseline.clone(),
                reader: self.set.groups()[entry.group_index].vcs(),
            })
            .collect();
        MemberVcsReaders { entries }
    }

    /// Borrow this opened set as the per-member release repo ports.
    ///
    /// Each member's single rskit-git adapter satisfies both the read-only
    /// guardrail port and the write port, so the same opened repo backs both
    /// halves of the [`MemberReleaseRepo`] binding.
    #[must_use]
    pub fn release_repos(&self) -> MemberReleaseRepos<'_> {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let vcs = self.set.groups()[entry.group_index].vcs();
                MemberReleaseRepo::new(entry.member.clone(), vcs, vcs)
            })
            .collect();
        MemberReleaseRepos::new(entries)
    }
}

/// One opened member reader as an index into [`VcsReaderSet`].
#[derive(Debug)]
struct OpenMemberVcsReader {
    member: Option<MemberId>,
    prefix: PathBuf,
    repo_prefix: PathBuf,
    baseline: Option<BaselineSpec>,
    group_index: usize,
}

/// Open one deduped reader set for every composed member repo.
///
/// # Errors
/// Propagates repository discovery/open failures or the impossible internal case
/// where an opened reader group cannot be matched back to its member placement.
pub fn open_member_vcs_readers(
    umbrella_root: &AbsPath,
    composed: &ComposedFederation,
    baselines: &MemberBaselines,
) -> AppResult<OpenMemberVcsReaders> {
    let placements = member_placements(composed)?;
    let set = VcsReaderSet::open(&placements)?;
    let mut entries = Vec::with_capacity(composed.members().len());
    for member in composed.members() {
        let placement_id = placement_id(member)?;
        let group_index = group_index_for(&set, &placement_id).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "opened VCS readers did not include member '{}'",
                    member.member().name()
                ),
            )
        })?;
        let baseline = baselines.get(member.member().id()).cloned();
        let group = &set.groups()[group_index];
        // The opened repo group resolved the actual git repo root; the placement
        // prefix is this member's discovery root *relative to that repo root*.
        // Stripping it rebases repo-root-relative change records down to
        // discovery-root-relative before the umbrella prefix is prepended.
        let repo_prefix = group
            .members()
            .iter()
            .find(|placement| placement.id() == &placement_id)
            .map(|placement| placement.prefix().to_path_buf())
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "opened VCS reader group for member '{}' did not include its placement",
                        member.member().name()
                    ),
                )
            })?;
        entries.push(OpenMemberVcsReader {
            member: member.member().id().cloned(),
            prefix: member_prefix(umbrella_root.as_path(), member.discover_root().as_path())?,
            repo_prefix,
            baseline,
            group_index,
        });
    }

    Ok(OpenMemberVcsReaders { set, entries })
}

/// Prefix repo-relative change records with a member's umbrella-relative path.
#[must_use]
pub(crate) fn prefix_records(records: &[ChangeRecord], prefix: &Path) -> Vec<ChangeRecord> {
    records
        .iter()
        .cloned()
        .map(|record| prefix_record(record, prefix))
        .collect()
}

fn prefix_record(mut record: ChangeRecord, prefix: &Path) -> ChangeRecord {
    if prefix.as_os_str().is_empty() {
        return record;
    }
    record.path = prefix.join(&record.path);
    record.old_path = record.old_path.map(|path| prefix.join(path));
    record
}

fn member_placements(composed: &ComposedFederation) -> AppResult<Vec<(WorkspaceId, PathBuf)>> {
    composed
        .members()
        .iter()
        .map(|member| {
            Ok((
                placement_id(member)?,
                member.discover_root().as_path().to_path_buf(),
            ))
        })
        .collect()
}

fn placement_id(member: &ComposedMember) -> AppResult<WorkspaceId> {
    let id = member
        .member()
        .id()
        .map_or_else(|| "root".to_string(), ToString::to_string);
    WorkspaceId::new(id)
}

fn group_index_for(set: &VcsReaderSet, placement_id: &WorkspaceId) -> Option<usize> {
    set.groups().iter().position(|group| {
        group
            .members()
            .iter()
            .any(|placement| placement.id() == placement_id)
    })
}

/// Borrowed per-member reader view consumed by affected selection.
pub struct MemberVcsReaders<'a> {
    entries: Vec<MemberVcsReader<'a>>,
}

impl<'a> MemberVcsReaders<'a> {
    /// Build a reader view from explicit member entries.
    ///
    /// Tests and non-git hosts can use this constructor with fake readers while
    /// production code normally uses [`OpenMemberVcsReaders::readers`].
    #[must_use]
    pub const fn new(entries: Vec<MemberVcsReader<'a>>) -> Self {
        Self { entries }
    }

    /// Build the degenerate single-repo reader view: one entry at the umbrella
    /// root with no member id, an empty prefix, and `baseline`.
    #[must_use]
    pub fn single(reader: &'a dyn VcsReader, baseline: BaselineSpec) -> Self {
        Self::new(vec![MemberVcsReader::new(
            None,
            PathBuf::new(),
            Some(baseline),
            reader,
        )])
    }

    /// The readers in member declaration order.
    #[must_use]
    pub fn entries(&self) -> &[MemberVcsReader<'a>] {
        &self.entries
    }
}

/// One member's VCS reader plus the path prefix that maps repo-relative changes
/// into umbrella-relative graph coordinates.
pub struct MemberVcsReader<'a> {
    member: Option<MemberId>,
    prefix: PathBuf,
    repo_prefix: PathBuf,
    baseline: Option<BaselineSpec>,
    reader: &'a dyn VcsReader,
}

impl<'a> MemberVcsReader<'a> {
    /// Construct one borrowed member reader entry.
    ///
    /// `baseline` is `None` when neither member config nor caller flags named a
    /// reference; a changed-selection over such a member then falls back to the
    /// request's baseline spec. The repo-root→discovery-root prefix defaults to
    /// empty (the member sits at its repo root); production wiring populates it
    /// from the opened repo group.
    #[must_use]
    pub fn new(
        member: Option<MemberId>,
        prefix: impl Into<PathBuf>,
        baseline: Option<BaselineSpec>,
        reader: &'a dyn VcsReader,
    ) -> Self {
        Self {
            member,
            prefix: prefix.into(),
            repo_prefix: PathBuf::new(),
            baseline,
            reader,
        }
    }

    /// The member this reader belongs to, or `None` for the degenerate project.
    #[must_use]
    pub const fn member(&self) -> Option<&MemberId> {
        self.member.as_ref()
    }

    /// The resolved baseline spec for this member, if one was named.
    #[must_use]
    pub const fn baseline(&self) -> Option<&BaselineSpec> {
        self.baseline.as_ref()
    }

    /// The VCS reader for this member repo.
    #[must_use]
    pub const fn reader(&self) -> &dyn VcsReader {
        self.reader
    }

    /// Map this member's repo-root-relative change records into the umbrella
    /// coordinate space the federated module graph uses.
    ///
    /// First strips the member's repo-root→discovery-root prefix (so records
    /// under a non-default `[project].root` become discovery-root-relative and
    /// changes outside the discovery root are dropped), then prepends the
    /// member's umbrella→discovery-root prefix. The net transform mirrors how
    /// member module roots are rebased into the federated graph, so prefixed
    /// records line up with module roots for change classification.
    #[must_use]
    pub fn umbrella_records(&self, records: &[ChangeRecord]) -> Vec<ChangeRecord> {
        let rebased = rebase_records(records, &self.repo_prefix);
        prefix_records(&rebased, &self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use toven_model::AbsPath;
    use toven_ports::BaselineMode;

    use super::resolve_baselines;
    use crate::config::{CanonicalRegistry, Document, MemberConfig, ProjectConfig, TovenConfig};
    use crate::federation::compose::compose_members;
    use crate::federation::members::enumerate_members;
    use crate::vcs::BaselineFlags;

    fn umbrella(members: Vec<MemberConfig>) -> Document {
        Document {
            project: ProjectConfig {
                name: "umbrella".to_string(),
                root: ".".to_string(),
                base_ref: Some("origin/main".to_string()),
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: BTreeMap::new(),
            members,
        }
    }

    fn member(name: &str, root: &str, base_ref: Option<&str>) -> MemberConfig {
        MemberConfig {
            name: name.to_string(),
            root: root.to_string(),
            base_ref: base_ref.map(str::to_owned),
        }
    }

    #[test]
    fn resolves_each_members_configured_baseline() {
        let ws = toven_testkit::workspace::workspace("member-baselines");
        ws.write_file(
            "repos/core/toven.toml",
            b"[project]\nname = \"core\"\nbase_ref = \"main\"\n",
        )
        .unwrap();
        ws.write_file(
            "repos/gateway/toven.toml",
            b"[project]\nname = \"gateway\"\nbase_ref = \"develop\"\n",
        )
        .unwrap();
        let root = AbsPath::new(ws.path().to_path_buf()).unwrap();
        let document = umbrella(vec![
            member("core", "repos/core", None),
            member("gateway", "repos/gateway", Some("release")),
        ]);
        let members = enumerate_members(&document, &root).unwrap();
        let composed = compose_members(
            &document,
            &members,
            &BTreeSet::new(),
            &CanonicalRegistry::model(),
        )
        .unwrap();

        let baselines = resolve_baselines(&composed, &BaselineFlags::new());

        assert_eq!(
            baselines
                .get(composed.members()[0].member().id())
                .unwrap()
                .reference,
            "main"
        );
        assert_eq!(
            baselines
                .get(composed.members()[1].member().id())
                .unwrap()
                .reference,
            "release"
        );
    }

    #[test]
    fn global_base_flag_applies_the_same_ref_name_to_every_member() {
        let ws = toven_testkit::workspace::workspace("member-baselines-global");
        ws.write_file(
            "repos/core/toven.toml",
            b"[project]\nname = \"core\"\nbase_ref = \"main\"\n",
        )
        .unwrap();
        ws.write_file(
            "repos/gateway/toven.toml",
            b"[project]\nname = \"gateway\"\nbase_ref = \"develop\"\n",
        )
        .unwrap();
        let root = AbsPath::new(ws.path().to_path_buf()).unwrap();
        let document = umbrella(vec![
            member("core", "repos/core", None),
            member("gateway", "repos/gateway", None),
        ]);
        let members = enumerate_members(&document, &root).unwrap();
        let composed = compose_members(
            &document,
            &members,
            &BTreeSet::new(),
            &CanonicalRegistry::model(),
        )
        .unwrap();
        let flags = BaselineFlags::new()
            .with_base("origin/trunk")
            .with_merge_base(true);

        let baselines = resolve_baselines(&composed, &flags);

        for member in composed.members() {
            let spec = baselines.get(member.member().id()).unwrap();
            assert_eq!(spec.reference, "origin/trunk");
            assert_eq!(spec.mode, BaselineMode::MergeBase);
        }
    }
}
