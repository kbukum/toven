//! Structural validation of a parsed [`Document`].
//!
//! Structural = the Load-phase contract: the document is well-formed, reserved
//! sections are typed (serde did that), and every `ecosystem:module` reference is
//! *syntactically* valid against a canonical ecosystem. Semantic resolution —
//! whether refs point at real modules and the graph is acyclic — is deferred to
//! the engine Graph phase.

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::{
    reject_unicode_controls, validate_path_safe_identifier, validate_required_trimmed,
    validate_safe_path,
};
use std::collections::BTreeSet;
use toven_model::EcosystemId;

use super::{
    CanonicalRegistry, Document, GroupConfig, MemberConfig, ModuleRefSyntax, OverlayConfig,
    ProjectConfig,
};

/// Run the full structural-validation pass over `document`.
pub(super) fn structural(document: &Document, canonical: &CanonicalRegistry) -> AppResult<()> {
    validate_project(&document.project)?;
    let mut seen_members = BTreeSet::new();
    for member in &document.members {
        validate_member(member)?;
        if !seen_members.insert(member.name.as_str()) {
            return Err(AppError::invalid_input(
                "members.name",
                format!("duplicate member name '{}'", member.name),
            ));
        }
    }
    for (name, group) in &document.groups {
        validate_group(name, group, canonical)?;
    }
    for (index, overlay) in document.overlays.iter().enumerate() {
        validate_overlay(index, overlay, canonical)?;
    }
    Ok(())
}

fn validate_project(project: &ProjectConfig) -> AppResult<()> {
    validate_required_trimmed("project.name", &project.name)?;
    validate_relative_root("project.root", &project.root)?;
    if let Some(base_ref) = &project.base_ref {
        reject_unicode_controls("project.base_ref", base_ref)?;
    }
    Ok(())
}

fn validate_member(member: &MemberConfig) -> AppResult<()> {
    validate_path_safe_identifier("members.name", &member.name)?;
    validate_relative_root("members.root", &member.root)?;
    if let Some(base_ref) = &member.base_ref {
        reject_unicode_controls("members.base_ref", base_ref)?;
    }
    Ok(())
}

fn validate_group(name: &str, group: &GroupConfig, canonical: &CanonicalRegistry) -> AppResult<()> {
    validate_path_safe_identifier("groups.name", name)?;
    if let Some(ecosystem) = &group.ecosystem {
        require_canonical("groups.ecosystem", ecosystem, canonical)?;
    }
    let modules_field = format!("groups.{name}.modules");
    for module in &group.modules {
        ModuleRefSyntax::validate_membership(
            &modules_field,
            module,
            group.ecosystem.as_ref(),
            canonical,
        )?;
    }
    let forbid_field = format!("groups.{name}.guardrails.forbid");
    for edge in &group.guardrails.forbid {
        ModuleRefSyntax::validate_qualified(&forbid_field, edge, canonical)?;
    }
    let allow_field = format!("groups.{name}.guardrails.allow");
    for edge in &group.guardrails.allow {
        ModuleRefSyntax::validate_qualified(&allow_field, edge, canonical)?;
    }
    Ok(())
}

fn validate_overlay(
    index: usize,
    overlay: &OverlayConfig,
    canonical: &CanonicalRegistry,
) -> AppResult<()> {
    require_canonical(
        &format!("overlays[{index}].from.ecosystem"),
        &overlay.from.ecosystem,
        canonical,
    )?;
    validate_path_safe_identifier(
        &format!("overlays[{index}].from.module"),
        &overlay.from.module,
    )?;
    require_canonical(
        &format!("overlays[{index}].to.ecosystem"),
        &overlay.to.ecosystem,
        canonical,
    )?;
    validate_path_safe_identifier(&format!("overlays[{index}].to.module"), &overlay.to.module)?;
    Ok(())
}

/// Validate a workspace-relative root: either `.` (the config-file directory) or
/// a safe relative path with no traversal.
fn validate_relative_root(field: &str, root: &str) -> AppResult<()> {
    if root == "." {
        return Ok(());
    }
    validate_safe_path(root).map_err(|error| AppError::invalid_input(field, error.to_string()))
}

fn require_canonical(
    field: &str,
    ecosystem: &EcosystemId,
    canonical: &CanonicalRegistry,
) -> AppResult<()> {
    if canonical.contains(ecosystem) {
        Ok(())
    } else {
        Err(AppError::invalid_input(
            field,
            format!("unknown ecosystem '{ecosystem}'"),
        ))
    }
}
