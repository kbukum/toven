//! Native version-reference sync: keep declared files' embedded version tokens
//! in lock-step with the authoritative post-bump versions.
//!
//! This is the deterministic, no-shell half of the version-reference feature. A
//! [`VersionReferenceConfig`](toven_ports::VersionReferenceConfig) declares file
//! globs and a per-line pattern (a [`Template`] over `{module}`/`{version}`);
//! [`sync_version_references`] rewrites only the `{version}` token of each
//! pattern-matching line to the authoritative version of the captured
//! `{module}`. The rewrite is format-preserving (only the matched token
//! changes), anchored per line (prose and examples that do not match the pattern
//! are never touched), and idempotent (a file already at the authoritative
//! versions is left byte-for-byte unchanged and contributes no staged path).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir;
use rskit_fs::sync_io::file::{read_string_bounded, write_atomic};
use rskit_util::glob::glob_match;
use rskit_util::template::{Template, TemplatePart};
use rskit_version::semver::Version;
use toven_model::{Module, ModuleKey, RepoPath};
use toven_ports::{VersionRefToken, VersionReferenceConfig};

use crate::ReleasePlan;

/// Upper bound on a version-reference file read; a document larger than this is
/// treated as malformed rather than loaded unbounded.
const MAX_REFERENCE_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for the atomic version-reference rewrite.
const REFERENCE_TEMP_PREFIX: &str = "toven-version-ref";

#[derive(Debug, Clone, PartialEq, Eq)]
enum AliasStatus {
    Unique(Option<Version>),
    Collided,
}

/// Build the collision-free `module → post-bump version` map from a plan's
/// entries.
///
/// Each planned or existing module contributes its canonical member-qualified
/// key ([`ModuleKey`] `Display`: `member/ecosystem:name`, or `ecosystem:name` in
/// the single-repo case) when it has a resolved version. The convenience
/// aliases (package name, `ecosystem:name`, and bare name) are added **only when
/// unambiguous** — an alias claimed by two or more modules in the plan (such as
/// two members exposing `core`, or a versioned module sharing a name with a
/// versionless module) is omitted rather than overwritten, so a version reference
/// never rewrites to a wrong version through a colliding alias.
#[must_use]
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn authoritative_versions(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
) -> BTreeMap<String, Version> {
    let mut canonical = BTreeMap::new();
    let mut alias_owners: BTreeMap<String, AliasStatus> = BTreeMap::new();
    for entry in &plan.entries {
        let version = entry
            .planned_version
            .clone()
            .or_else(|| entry.current_version.clone());
        let Some(module) = module_by_ref.get(&entry.module) else {
            continue;
        };
        if let Some(version) = &version {
            canonical.insert(entry.module.to_string(), version.clone());
        }
        let mut aliases = vec![module.id.to_string(), module.id.name.clone()];
        if let Some(package) = &module.package {
            aliases.push(package.clone());
        }
        for alias in aliases {
            alias_owners
                .entry(alias)
                .and_modify(|status| *status = AliasStatus::Collided)
                .or_insert_with(|| AliasStatus::Unique(version.clone()));
        }
    }
    for (alias, status) in alias_owners {
        // Keep an alias only when a single module claimed it, that module has a
        // resolved version, and never let it shadow a canonical member-qualified key.
        if let AliasStatus::Unique(Some(version)) = status {
            canonical.entry(alias).or_insert(version);
        }
    }
    canonical
}

/// Rewrite every declared version reference under `root` to the authoritative
/// versions, returning the repo-relative paths actually changed (sorted, unique).
///
/// A file whose tokens already match is not rewritten and contributes no path,
/// so a re-run stages nothing. `references` is the repo-scoped union of a
/// member's declarations. A file matched by several declarations (or overlapping
/// globs) is read and written **once**, with every matching pattern applied to
/// its content in declaration order.
///
/// # Errors
/// Rejects a malformed pattern or unsafe glob, and propagates a read/write
/// failure on a matched file.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn sync_version_references(
    references: &[VersionReferenceConfig],
    versions: &BTreeMap<String, Version>,
    root: &Path,
) -> AppResult<Vec<RepoPath>> {
    let mut templates_by_file: BTreeMap<String, Vec<Template<VersionRefToken>>> = BTreeMap::new();
    for reference in references {
        let template = reference.parse_pattern()?;
        for glob in &reference.files {
            for relative in resolve_glob(root, glob)? {
                templates_by_file
                    .entry(relative)
                    .or_default()
                    .push(template.clone());
            }
        }
    }
    let mut changed = Vec::new();
    for (relative, templates) in templates_by_file {
        let absolute = safe_join(root, &relative).map_err(|error| {
            AppError::invalid_input(
                "release.version_references.files",
                format!(
                    "version-reference path '{relative}' is not a safe \
                     project-relative path"
                ),
            )
            .with_cause(error)
        })?;
        let text = read_string_bounded(&absolute, MAX_REFERENCE_BYTES).map_err(|error| {
            AppError::invalid_input(
                "release.version_references",
                format!("version-reference file '{relative}' could not be read"),
            )
            .with_cause(error)
        })?;
        let mut content = text;
        let mut any = false;
        for template in &templates {
            if let Some(rewritten) = rewrite_content(&content, template, versions) {
                content = rewritten;
                any = true;
            }
        }
        if any {
            write_atomic(&absolute, content.as_bytes(), REFERENCE_TEMP_PREFIX)?;
            changed.push(RepoPath::new(relative)?);
        }
    }
    Ok(changed)
}

/// Resolve a repo-relative `*`/`?` glob to the matching existing **file** paths
/// under `root`, expanding one path segment at a time.
///
/// A wildcard-free glob resolves to at most the single literal file (when it
/// exists); a segment with a wildcard is expanded against that directory's
/// listing. Expansion never escapes `root` (a `..`/absolute segment is rejected
/// when the resulting path is validated as a [`RepoPath`]).
fn resolve_glob(root: &Path, glob: &str) -> AppResult<Vec<String>> {
    let mut candidates = vec![PathBuf::new()];
    for segment in glob.split('/') {
        if segment.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for candidate in &candidates {
            let dir_abs = root.join(candidate);
            if segment_has_wildcard(segment) {
                if !dir::exists(&dir_abs)? {
                    continue;
                }
                for entry in dir::list(&dir_abs)? {
                    if glob_match(segment, &entry.file_name.to_string_lossy()) {
                        next.push(candidate.join(&entry.file_name));
                    }
                }
            } else {
                next.push(candidate.join(segment));
            }
        }
        candidates = next;
    }
    let mut files = Vec::new();
    for candidate in candidates {
        let absolute = root.join(&candidate);
        if absolute.is_file() {
            files.push(candidate.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(files)
}

/// Whether a glob segment carries a `*`/`?` wildcard.
fn segment_has_wildcard(segment: &str) -> bool {
    segment.contains('*') || segment.contains('?')
}

/// Rewrite every pattern-matching line of `content`, returning the new content
/// when any token changed and `None` when nothing changed.
fn rewrite_content(
    content: &str,
    template: &Template<VersionRefToken>,
    versions: &BTreeMap<String, Version>,
) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(content.len());
    let mut remaining = content;
    while !remaining.is_empty() {
        let (line, rest, newline) = split_line(remaining);
        remaining = rest;
        match rewrite_line(line, template, versions) {
            Some(rewritten) => {
                changed = true;
                out.push_str(&rewritten);
            }
            None => out.push_str(line),
        }
        out.push_str(newline);
    }
    changed.then_some(out)
}

/// Split `text` into its first line, the remainder, and the line's terminator
/// (`"\n"`, `"\r\n"`, or `""` at end of input).
fn split_line(text: &str) -> (&str, &str, &str) {
    text.find('\n').map_or((text, "", ""), |index| {
        let (line_with_cr, rest) = text.split_at(index);
        let rest = &rest[1..];
        line_with_cr
            .strip_suffix('\r')
            .map_or((line_with_cr, rest, "\n"), |stripped| {
                (stripped, rest, "\r\n")
            })
    })
}

/// Rewrite a single line when it matches the pattern and the captured module has
/// an authoritative version differing from the line's current token.
fn rewrite_line(
    line: &str,
    template: &Template<VersionRefToken>,
    versions: &BTreeMap<String, Version>,
) -> Option<String> {
    let capture = match_line(line, template)?;
    let module = &line[capture.module.clone()];
    let version = versions.get(module)?;
    let current = &line[capture.version.clone()];
    let replacement = version.to_string();
    if current == replacement {
        return None;
    }
    let mut rewritten = String::with_capacity(line.len());
    rewritten.push_str(&line[..capture.version.start]);
    rewritten.push_str(&replacement);
    rewritten.push_str(&line[capture.version.end..]);
    Some(rewritten)
}

/// Byte ranges captured by a matched line: the `{module}` and `{version}` spans.
struct LineCapture {
    module: std::ops::Range<usize>,
    version: std::ops::Range<usize>,
}

/// Match `line` against the template, anchored after leading whitespace and
/// required to consume the whole line. Returns the `{module}`/`{version}` byte
/// spans on a full match.
fn match_line(line: &str, template: &Template<VersionRefToken>) -> Option<LineCapture> {
    let lead = line.len() - line.trim_start().len();
    let mut pos = lead;
    let mut module: Option<std::ops::Range<usize>> = None;
    let mut version: Option<std::ops::Range<usize>> = None;
    let parts = template.parts();
    for (index, part) in parts.iter().enumerate() {
        match part {
            TemplatePart::Literal(literal) => {
                if !line[pos..].starts_with(literal.as_str()) {
                    return None;
                }
                pos += literal.len();
            }
            TemplatePart::Placeholder(token) => {
                let next_literal = match parts.get(index + 1) {
                    Some(TemplatePart::Literal(literal)) => Some(literal.as_str()),
                    _ => None,
                };
                let end = match next_literal {
                    Some(literal) => pos + line[pos..].find(literal)?,
                    None => line.len(),
                };
                let captured = &line[pos..end];
                if !token_matches(*token, captured) {
                    return None;
                }
                match token {
                    VersionRefToken::Module => module = Some(pos..end),
                    VersionRefToken::Version => version = Some(pos..end),
                }
                pos = end;
            }
        }
    }
    if pos != line.len() {
        return None;
    }
    Some(LineCapture {
        module: module?,
        version: version?,
    })
}

/// Whether a captured span is well-formed for its placeholder: a non-empty
/// identifier for `{module}`, a semver-shaped token (digit-led, semver charset)
/// for `{version}`.
fn token_matches(token: VersionRefToken, captured: &str) -> bool {
    if captured.is_empty() {
        return false;
    }
    match token {
        VersionRefToken::Module => captured
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.:-".contains(character)),
        VersionRefToken::Version => {
            captured.starts_with(|character: char| character.is_ascii_digit())
                && captured
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rewrite_content;
    use rskit_version::semver::Version;
    use std::collections::BTreeMap;
    use toven_ports::VersionReferenceConfig;

    fn template(pattern: &str) -> rskit_util::template::Template<toven_ports::VersionRefToken> {
        VersionReferenceConfig {
            files: vec!["README.md".into()],
            pattern: pattern.into(),
        }
        .parse_pattern()
        .expect("pattern parses")
    }

    fn versions(entries: &[(&str, (u64, u64, u64))]) -> BTreeMap<String, Version> {
        entries
            .iter()
            .map(|(name, (major, minor, patch))| {
                ((*name).to_string(), Version::new(*major, *minor, *patch))
            })
            .collect()
    }

    #[test]
    fn a_matching_pin_is_rewritten_to_the_authoritative_version() {
        let content = "toven-rust = \"0.1.0\"\n";
        let rewritten = rewrite_content(
            content,
            &template("{module} = \"{version}\""),
            &versions(&[("toven-rust", (0, 2, 0))]),
        )
        .expect("content changed");
        assert_eq!(rewritten, "toven-rust = \"0.2.0\"\n");
    }

    #[test]
    fn an_already_current_pin_is_left_unchanged() {
        let content = "toven-rust = \"0.2.0\"\n";
        assert!(
            rewrite_content(
                content,
                &template("{module} = \"{version}\""),
                &versions(&[("toven-rust", (0, 2, 0))]),
            )
            .is_none(),
            "an idempotent sync must report no change"
        );
    }

    #[test]
    fn a_module_absent_from_the_map_is_left_untouched() {
        let content = "other = \"9.9.9\"\n";
        assert!(
            rewrite_content(
                content,
                &template("{module} = \"{version}\""),
                &versions(&[("toven-rust", (0, 2, 0))]),
            )
            .is_none(),
            "a pin for an unbumped module is untouched"
        );
    }

    #[test]
    fn prose_that_does_not_match_the_pattern_is_untouched() {
        // A prose line mentioning a version-shaped string but not matching the
        // whole pin pattern must never be rewritten.
        let content = "We shipped toven-rust 0.1.0 last week.\n";
        assert!(
            rewrite_content(
                content,
                &template("{module} = \"{version}\""),
                &versions(&[("toven-rust", (0, 2, 0))]),
            )
            .is_none(),
            "prose is anchored out of the rewrite"
        );
    }

    #[test]
    fn only_matching_lines_change_and_indentation_is_preserved() {
        let content = "intro\n    toven-rust = \"0.1.0\"\ntrailer\n";
        let rewritten = rewrite_content(
            content,
            &template("{module} = \"{version}\""),
            &versions(&[("toven-rust", (0, 2, 0))]),
        )
        .expect("content changed");
        assert_eq!(rewritten, "intro\n    toven-rust = \"0.2.0\"\ntrailer\n");
    }

    #[test]
    fn cross_module_pins_resolve_against_one_map() {
        let content = "core = \"0.1.0\"\napp = \"1.0.0\"\n";
        let rewritten = rewrite_content(
            content,
            &template("{module} = \"{version}\""),
            &versions(&[("core", (0, 2, 0)), ("app", (1, 1, 0))]),
        )
        .expect("content changed");
        assert_eq!(rewritten, "core = \"0.2.0\"\napp = \"1.1.0\"\n");
    }

    #[test]
    fn a_line_without_a_trailing_newline_is_rewritten() {
        let content = "toven-rust = \"0.1.0\"";
        let rewritten = rewrite_content(
            content,
            &template("{module} = \"{version}\""),
            &versions(&[("toven-rust", (0, 2, 0))]),
        )
        .expect("content changed");
        assert_eq!(rewritten, "toven-rust = \"0.2.0\"");
    }

    #[test]
    fn authoritative_versions_maps_canonical_and_unambiguous_aliases() {
        use super::authoritative_versions;
        use crate::{
            BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseEntry,
            ReleasePlan,
        };
        use toven_model::{EcosystemId, MemberId, Module, ModuleKey, ModuleRef, RepoPath};
        use toven_ports::{BumpLevel, ReleaseMutation};

        let shared_key = ModuleKey::new(
            Some(MemberId::new("core").unwrap()),
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "shared").unwrap(),
        );
        let mut shared_mod = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "shared").unwrap(),
            RepoPath::new("crates/shared").unwrap(),
        );
        shared_mod.member = Some(MemberId::new("core").unwrap());
        shared_mod.package = Some("core-shared".to_string());

        let api_key = ModuleKey::new(
            Some(MemberId::new("gateway").unwrap()),
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "api").unwrap(),
        );
        let mut api_mod = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "api").unwrap(),
            RepoPath::new("crates/api").unwrap(),
        );
        api_mod.member = Some(MemberId::new("gateway").unwrap());

        let entry_shared = ReleaseEntry {
            module: shared_key.clone(),
            current_version: Some(Version::new(0, 1, 0)),
            planned_version: Some(Version::new(0, 2, 0)),
            planned_tag: None,
            level: BumpLevel::Minor,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(Version::new(0, 2, 0)),
            publication: toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into(),
            },
            publish_needed: false,
            tag_format: None,
            tag_mode: None,
            baseline_source: None,
            tag_message: None,
            signer: None,
            commit_message: None,
            token_env: None,
            visibility: toven_ports::Visibility::Public,
            push: PushPolicy::BranchAndTags,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(shared_key.clone(), "changed", Vec::new()),
            changelog_path: "CHANGELOG.md".into(),
            changelog_roll: false,
            entrypoint: toven_model::Entrypoint::Toven,
            umbrella: false,
            version_references: Vec::new(),
            on_resolved: Vec::new(),
        };

        let mut entry_api = entry_shared.clone();
        entry_api.module = api_key.clone();
        entry_api.planned_version = Some(Version::new(1, 1, 0));

        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry_shared, entry_api]);
        let module_by_ref: BTreeMap<ModuleKey, &Module> =
            BTreeMap::from([(shared_key, &shared_mod), (api_key, &api_mod)]);

        let map = authoritative_versions(&plan, &module_by_ref);
        assert_eq!(map.get("core/rust:shared"), Some(&Version::new(0, 2, 0)));
        assert_eq!(map.get("core-shared"), Some(&Version::new(0, 2, 0)));
        assert_eq!(map.get("gateway/rust:api"), Some(&Version::new(1, 1, 0)));
        assert_eq!(map.get("api"), Some(&Version::new(1, 1, 0)));
    }

    #[test]
    fn authoritative_versions_drops_aliases_colliding_with_versionless_entries() {
        use super::authoritative_versions;
        use crate::{
            BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseEntry,
            ReleasePlan,
        };
        use toven_model::{EcosystemId, MemberId, Module, ModuleKey, ModuleRef, RepoPath};
        use toven_ports::{BumpLevel, ReleaseMutation};

        let rust_key = ModuleKey::new(
            Some(MemberId::new("rust-side").unwrap()),
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
        );
        let mut rust_mod = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
            RepoPath::new("crates/core").unwrap(),
        );
        rust_mod.member = Some(MemberId::new("rust-side").unwrap());

        let go_key = ModuleKey::new(
            Some(MemberId::new("go-side").unwrap()),
            ModuleRef::new(EcosystemId::new("go").unwrap(), "core").unwrap(),
        );
        let mut go_mod = Module::new(
            ModuleRef::new(EcosystemId::new("go").unwrap(), "core").unwrap(),
            RepoPath::new("modules/core").unwrap(),
        );
        go_mod.member = Some(MemberId::new("go-side").unwrap());

        let entry_rust = ReleaseEntry {
            module: rust_key.clone(),
            current_version: Some(Version::new(0, 1, 0)),
            planned_version: Some(Version::new(1, 0, 0)),
            planned_tag: None,
            level: BumpLevel::Major,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(Version::new(1, 0, 0)),
            publication: toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into(),
            },
            publish_needed: false,
            tag_format: None,
            tag_mode: None,
            baseline_source: None,
            tag_message: None,
            signer: None,
            commit_message: None,
            token_env: None,
            visibility: toven_ports::Visibility::Public,
            push: PushPolicy::BranchAndTags,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(rust_key.clone(), "changed", Vec::new()),
            changelog_path: "CHANGELOG.md".into(),
            changelog_roll: false,
            entrypoint: toven_model::Entrypoint::Toven,
            umbrella: false,
            version_references: Vec::new(),
            on_resolved: Vec::new(),
        };

        let mut entry_go = entry_rust.clone();
        entry_go.module = go_key.clone();
        entry_go.current_version = None;
        entry_go.planned_version = None;

        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![entry_rust, entry_go]);
        let module_by_ref: BTreeMap<ModuleKey, &Module> =
            BTreeMap::from([(rust_key, &rust_mod), (go_key, &go_mod)]);

        let map = authoritative_versions(&plan, &module_by_ref);
        assert_eq!(
            map.get("rust-side/rust:core"),
            Some(&Version::new(1, 0, 0)),
            "canonical member-qualified key must be retained"
        );
        assert_eq!(
            map.get("core"),
            None,
            "bare name 'core' collided with the versionless module and must be dropped"
        );
    }
}
