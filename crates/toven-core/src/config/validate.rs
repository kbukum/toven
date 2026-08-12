//! Structural validation of a parsed [`Document`].
//!
//! Structural = the Load-phase contract: the document is well-formed, reserved
//! sections are typed (serde did that), and every `ecosystem:module` reference
//! is *syntactically* valid against a canonical ecosystem. Semantic resolution
//! — whether refs point at real modules and the graph is acyclic — is deferred
//! to the engine Graph phase.

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::{
    reject_unicode_controls, validate_path_safe_identifier, validate_required_trimmed,
    validate_safe_path,
};
use std::collections::BTreeSet;
use toven_model::EcosystemId;

use super::{
    CanonicalRegistry, Document, GroupConfig, MemberConfig, ModuleRefSyntax, OverlayConfig,
    ProjectConfig, TovenConfig,
};

/// Run the full structural-validation pass over `document`.
pub(super) fn structural(document: &Document, canonical: &CanonicalRegistry) -> AppResult<()> {
    validate_project(&document.project)?;
    validate_settings(&document.toven)?;
    let mut seen_members = BTreeSet::new();
    for (index, member) in document.members.iter().enumerate() {
        validate_member(index, member)?;
        if !seen_members.insert(member.name.as_str()) {
            return Err(AppError::invalid_input(
                format!("members[{index}].name"),
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
    for (reference, module) in &document.modules {
        validate_module(reference, module, canonical)?;
    }
    for (verb, hooks) in &document.hooks {
        hooks.validate(&format!("hooks.{}", verb.as_str()))?;
    }
    super::units::validate_units(&document.units)?;
    Ok(())
}

/// Validate one `[modules.<ecosystem:module>]` override: the key is a canonical
/// `ecosystem:module` reference and its release and coverage overrides are
/// field-valid.
fn validate_module(
    reference: &str,
    module: &super::ModuleConfig,
    canonical: &CanonicalRegistry,
) -> AppResult<()> {
    ModuleRefSyntax::validate_qualified(&format!("modules.{reference}"), reference, canonical)?;
    module
        .release
        .validate(&format!("modules.{reference}.release"))?;
    module
        .coverage
        .validate_module_override(&format!("modules.{reference}.coverage"))
}

fn validate_project(project: &ProjectConfig) -> AppResult<()> {
    validate_required_trimmed("project.name", &project.name)?;
    validate_relative_root("project.root", &project.root)?;
    if let Some(base_ref) = &project.base_ref {
        reject_unicode_controls("project.base_ref", base_ref)?;
    }
    Ok(())
}

/// Validate the reserved `[toven]` settings.
///
/// `[toven.cache].dir` is a workspace-relative cache-root override consumed
/// later by the engine cache layer for filesystem paths, so it must be a safe
/// relative path (no traversal/absolute escape) — validated here at the trust
/// boundary.
fn validate_settings(settings: &TovenConfig) -> AppResult<()> {
    if let Some(dir) = &settings.cache.dir {
        validate_safe_path(dir).map_err(|error| {
            AppError::invalid_input("toven.cache.dir", error.to_string()).with_cause(error)
        })?;
    }
    Ok(())
}

fn validate_member(index: usize, member: &MemberConfig) -> AppResult<()> {
    validate_path_safe_identifier(&format!("members[{index}].name"), &member.name)?;
    reject_unit_id_separator(&format!("members[{index}].name"), &member.name)?;
    validate_relative_root(&format!("members[{index}].root"), &member.root)?;
    if let Some(base_ref) = &member.base_ref {
        reject_unicode_controls(&format!("members[{index}].base_ref"), base_ref)?;
    }
    Ok(())
}

/// The reserved unit-id separator Toven folds group override identities and
/// dependency layers behind (`base~identity`, `base~~L{layer}`). It must never
/// appear in a user identifier that reaches a unit id, or a group/member name
/// could shadow the scheduler's layer marker and trip the distinct-id guard on
/// otherwise valid config.
const UNIT_ID_SEPARATOR: char = '~';

/// Reject the reserved [`UNIT_ID_SEPARATOR`] in a user identifier that is
/// folded into a unit id (group and member names).
fn reject_unit_id_separator(field: &str, value: &str) -> AppResult<()> {
    if value.contains(UNIT_ID_SEPARATOR) {
        return Err(AppError::invalid_input(
            field,
            format!("cannot contain the reserved '{UNIT_ID_SEPARATOR}' character"),
        ));
    }
    Ok(())
}

fn validate_group(name: &str, group: &GroupConfig, canonical: &CanonicalRegistry) -> AppResult<()> {
    validate_path_safe_identifier(&format!("groups.{name}"), name)?;
    reject_unit_id_separator(&format!("groups.{name}"), name)?;
    if let Some(ecosystem) = &group.ecosystem {
        require_canonical(&format!("groups.{name}.ecosystem"), ecosystem, canonical)?;
    }
    for (index, module) in group.modules.iter().enumerate() {
        ModuleRefSyntax::validate_membership(
            &format!("groups.{name}.modules[{index}]"),
            module,
            group.ecosystem.as_ref(),
            canonical,
        )?;
    }
    for (index, edge) in group.guardrails.forbid.iter().enumerate() {
        ModuleRefSyntax::validate_qualified(
            &format!("groups.{name}.guardrails.forbid[{index}]"),
            edge,
            canonical,
        )?;
    }
    for (index, edge) in group.guardrails.allow.iter().enumerate() {
        ModuleRefSyntax::validate_qualified(
            &format!("groups.{name}.guardrails.allow[{index}]"),
            edge,
            canonical,
        )?;
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

/// Validate a workspace-relative root: either `.` (the config-file directory)
/// or a safe relative path with no traversal.
fn validate_relative_root(field: &str, root: &str) -> AppResult<()> {
    if root == "." {
        return Ok(());
    }
    validate_safe_path(root)
        .map_err(|error| AppError::invalid_input(field, error.to_string()).with_cause(error))
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
