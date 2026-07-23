//! Minimal first-run emit: assemble `[project]` and the detected
//! `[ecosystems.<id>]` sections with a few commented override hints, relying on
//! smart defaults for everything else.

use rskit_errors::{AppError, AppResult, ErrorCode};
use toml_edit::{DocumentMut, Item, Table, value};
use toven_model::EcosystemId;
use toven_ports::EcosystemFragment;

/// The generic, ecosystem-agnostic override hints prefixed to each scaffolded
/// `[ecosystems.<id>]` section. Only the discovery hints are emitted live;
/// everything else is filled by smart defaults, so these stay commented.
const SECTION_HINTS: &str = "\n# Smart defaults fill in tasks, run strategy, and toolchain probes.\n# Uncomment to override, e.g.:\n#   run_strategy = \"leaf-to-top\"\n";

/// Render a fresh, minimal `toven.toml` for `project_name` carrying every
/// detected `fragment`.
///
/// Returns the rendered text and the ids of the sections it wrote (all of them,
/// on a first run).
///
/// # Errors
/// Returns an error only if a fragment table cannot be re-encoded as TOML.
pub(super) fn first_run(
    project_name: &str,
    base_ref: &str,
    fragments: &[EcosystemFragment],
) -> AppResult<(String, Vec<EcosystemId>)> {
    let mut doc = DocumentMut::new();
    doc.insert(
        "project",
        Item::Table(project_table(project_name, base_ref)),
    );

    let mut added = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        insert_section(&mut doc, fragment)?;
        added.push(fragment.ecosystem.clone());
    }

    Ok((doc.to_string(), added))
}

/// Build the minimal `[project]` table: name, the conventional `.` root, and
/// the resolved change baseline (`base_ref`, detected from the repository's
/// remotes/branches by the caller).
fn project_table(project_name: &str, base_ref: &str) -> Table {
    let mut project = Table::new();
    project.insert("name", value(project_name));
    project.insert("root", value("."));
    project.insert("base_ref", value(base_ref));
    project
}

/// Splice one `[ecosystems.<id>]` section (with commented override hints) into
/// `doc`, creating the implicit `ecosystems` parent on demand.
///
/// The parent `ecosystems` table is kept *implicit* so it renders only the
/// `[ecosystems.<id>]` leaf headers, never a bare `[ecosystems]` header.
pub(super) fn insert_section(doc: &mut DocumentMut, fragment: &EcosystemFragment) -> AppResult<()> {
    let id = fragment.ecosystem.as_str();
    if doc.get("ecosystems").is_none() {
        let mut ecosystems = Table::new();
        ecosystems.set_implicit(true);
        doc.insert("ecosystems", Item::Table(ecosystems));
    }
    let ecosystems = doc["ecosystems"].as_table_mut().ok_or_else(|| {
        AppError::new(
            ErrorCode::InvalidInput,
            "`ecosystems` exists but is not a table; refusing to merge a section",
        )
    })?;
    ecosystems.insert(id, fragment_table_item(&fragment.table)?);
    if let Some(section) = ecosystems[id].as_table_mut() {
        section.set_implicit(false);
        section.decor_mut().set_prefix(SECTION_HINTS);
    }
    Ok(())
}

/// Convert a raw `[ecosystems.<id>]` body [`toml::Table`] into a format-aware
/// [`toml_edit`] table item by round-tripping through TOML text.
fn fragment_table_item(table: &toml::Table) -> AppResult<Item> {
    let text = toml::to_string(table).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to encode ecosystem fragment").with_cause(error)
    })?;
    let parsed: DocumentMut = text.parse().map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to parse ecosystem fragment").with_cause(error)
    })?;
    Ok(Item::Table(parsed.as_table().clone()))
}
