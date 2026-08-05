//! Per-member repository VCS handles shared by federation baseline and the
//! release flow. Release-agnostic: each handle is just a member's repo root
//! plus its read/write VCS ports.

use std::path::{Path, PathBuf};

use toven_model::MemberId;
use toven_ports::{VcsReader, VcsWriter};


/// One member repo's release VCS ports.
pub struct MemberReleaseRepo<'a> {
    member: Option<MemberId>,
    root: PathBuf,
    reader: &'a dyn VcsReader,
    writer: &'a dyn VcsWriter,
}

impl<'a> MemberReleaseRepo<'a> {
    /// Construct one member repo release port binding.
    ///
    /// `root` is the member's canonical repository root — the working directory
    /// forge commands (e.g. the hosted-release `gh` calls) run from, so a
    /// Release is cut against the repo whose tags this member pushed.
    #[must_use]
    pub fn new(
        member: Option<MemberId>,
        root: PathBuf,
        reader: &'a dyn VcsReader,
        writer: &'a dyn VcsWriter,
    ) -> Self {
        Self {
            member,
            root,
            reader,
            writer,
        }
    }

    /// The member this repo belongs to, or `None` for the degenerate project.
    #[must_use]
    pub const fn member(&self) -> Option<&MemberId> {
        self.member.as_ref()
    }

    /// The member's canonical repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read-only VCS port for guardrails.
    #[must_use]
    pub const fn reader(&self) -> &dyn VcsReader {
        self.reader
    }

    /// Write VCS port for commit/tag/push/restore.
    #[must_use]
    pub const fn writer(&self) -> &dyn VcsWriter {
        self.writer
    }
}

/// Member repo release ports in declaration order.
pub struct MemberReleaseRepos<'a> {
    entries: Vec<MemberReleaseRepo<'a>>,
}

impl<'a> MemberReleaseRepos<'a> {
    /// Construct a member repo release port set.
    #[must_use]
    pub const fn new(entries: Vec<MemberReleaseRepo<'a>>) -> Self {
        Self { entries }
    }

    pub(crate) fn get(&self, member: Option<&MemberId>) -> Option<&MemberReleaseRepo<'a>> {
        self.entries
            .iter()
            .find(|entry| entry.member.as_ref() == member)
    }

    /// The canonical repository root for `member`, if it is a known member
    /// repo.
    #[must_use]
    pub fn root_for(&self, member: Option<&MemberId>) -> Option<&Path> {
        self.get(member).map(MemberReleaseRepo::root)
    }

    /// The read-only VCS port for `member`, if it is a known member repo.
    ///
    /// The reconcile pre-pass uses it to confirm that a published version's
    /// release tag exists before completing its missing hosted Release.
    #[must_use]
    pub fn reader_for(&self, member: Option<&MemberId>) -> Option<&dyn VcsReader> {
        self.get(member).map(MemberReleaseRepo::reader)
    }
}
