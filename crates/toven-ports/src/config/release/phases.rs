//! Per-phase backing vocabulary: how each release [`ReleasePhase`] is satisfied
//! (`[…release.phases.<phase>]`).
//!
//! This is the **contract sketch** for the per-phase seam: it names, per phase,
//! whether Toven backs the phase natively (the default) or delegates it to an
//! external tool invoked argv-first. It resolves to the seam-level
//! [`PhaseBacking`](crate::release::PhaseBacking). The engine wiring that reads
//! this — resolution into settings and per-phase execution — lands with the
//! phase seam refactor; until [`ReleaseConfig`](super::ReleaseConfig) includes
//! this field, the strict loader still rejects `[…release.phases]` and every
//! phase runs natively.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use toven_model::ReleasePhase;

use crate::release::PhaseBacking;

/// The `[…release.phases]` sub-config: a per-phase backing map.
///
/// Each entry backs one [`ReleasePhase`] either natively (the default, so an
/// absent entry means native) or by delegating to an external tool. Unknown
/// phase names are a typed parse error, never a silently-ignored key.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct PhasesConfig(pub BTreeMap<ReleasePhase, PhaseConfig>);

impl PhasesConfig {
    /// Whether no phase backing is configured (so it can be skipped on
    /// serialize).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The resolved backing for `phase` — [`PhaseBacking::Native`] when the
    /// phase has no configured entry.
    ///
    /// `field` is the config path prefix used in diagnostics (e.g.
    /// `ecosystems.go.release.phases`).
    ///
    /// # Errors
    /// Propagates [`PhaseConfig::resolve`] for a configured but inconsistent
    /// entry.
    pub fn backing(&self, phase: ReleasePhase, field: &str) -> AppResult<PhaseBacking> {
        self.0.get(&phase).map_or_else(
            || Ok(PhaseBacking::Native),
            |config| config.resolve(&format!("{field}.{}", phase.as_str())),
        )
    }

    /// Validate every configured phase entry.
    ///
    /// # Errors
    /// Rejects a phase whose backing selection is inconsistent with its
    /// delegated-tool sub-block (see [`PhaseConfig::validate`]).
    pub fn validate(&self, field: &str) -> AppResult<()> {
        for (phase, config) in &self.0 {
            config.validate(&format!("{field}.{}", phase.as_str()))?;
        }
        Ok(())
    }
}

/// A single phase's backing (`[…release.phases.<phase>]`).
///
/// `backing` selects native (default) or delegated; a `delegated` sub-block
/// names the external tool. The two must agree: a `delegated` backing requires
/// the tool sub-block, and a `native` backing must not carry one.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseConfig {
    /// Whether the phase is backed natively (default) or delegated.
    #[serde(default, skip_serializing_if = "PhaseBackingKind::is_native")]
    pub backing: PhaseBackingKind,
    /// The delegated external tool; required when `backing = "delegated"`,
    /// forbidden otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated: Option<DelegatedTool>,
}

impl PhaseConfig {
    /// Resolve this entry into a seam-level [`PhaseBacking`], validating it
    /// first so an inconsistent selection surfaces as a typed error rather than
    /// a success-shaped default.
    ///
    /// `field` is the config path prefix used in diagnostics.
    ///
    /// # Errors
    /// Propagates [`validate`](Self::validate): a `delegated` backing without a
    /// tool sub-block, or a tool sub-block under a `native` backing, is
    /// rejected instead of silently coerced to [`PhaseBacking::Native`].
    pub fn resolve(&self, field: &str) -> AppResult<PhaseBacking> {
        self.validate(field)?;
        // After `validate`, a tool sub-block is present exactly when the backing
        // is delegated, so the tool's presence faithfully decides the backing.
        Ok(self
            .delegated
            .as_ref()
            .map_or(PhaseBacking::Native, |tool| {
                PhaseBacking::delegated(&tool.tool)
            }))
    }

    /// Validate the backing selection against the delegated sub-block.
    ///
    /// # Errors
    /// Rejects a `delegated` backing without a tool sub-block, a `native`
    /// backing that nonetheless carries one, and a blank tool or blank tool
    /// argument.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        match (self.backing, &self.delegated) {
            (PhaseBackingKind::Delegated, None) => Err(AppError::invalid_input(
                format!("{field}.delegated"),
                "backing = \"delegated\" requires a [delegated] tool sub-block",
            )),
            (PhaseBackingKind::Native, Some(_)) => Err(AppError::invalid_input(
                format!("{field}.delegated"),
                "a delegated tool is set but backing is native (set backing = \"delegated\")",
            )),
            (PhaseBackingKind::Delegated, Some(tool)) => {
                tool.validate(&format!("{field}.delegated"))
            }
            (PhaseBackingKind::Native, None) => Ok(()),
        }
    }
}

/// Whether a phase is backed natively or by a delegated external tool.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PhaseBackingKind {
    /// Toven implements the phase itself — the default.
    #[default]
    Native,
    /// Toven delegates the phase to the external tool named in the `delegated`
    /// sub-block.
    Delegated,
}

impl PhaseBackingKind {
    /// Whether this is the native (default) backing.
    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }
}

/// The delegated external tool for a phase (`[…release.phases.<phase>.delegated]`).
///
/// The tool is invoked argv-first: `tool` names the executable and `args` are
/// fixed leading arguments. It carries no secrets — those flow through the
/// child-process environment, never argv.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedTool {
    /// The external executable that backs the phase (e.g. `goreleaser`).
    pub tool: String,
    /// Fixed leading arguments passed to the tool, argv-first; `None` = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

impl DelegatedTool {
    /// Validate the tool selection.
    ///
    /// # Errors
    /// Rejects a blank tool name or any blank argument.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if self.tool.trim().is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.tool"),
                "tool must not be blank",
            ));
        }
        if let Some(args) = &self.args {
            for (index, arg) in args.iter().enumerate() {
                if arg.trim().is_empty() {
                    return Err(AppError::invalid_input(
                        format!("{field}.args[{index}]"),
                        "argument must not be blank",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use toven_model::ReleasePhase;

    use super::{PhaseBacking, PhaseBackingKind, PhasesConfig};

    fn parse(toml: &str) -> Result<PhasesConfig, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn empty_phases_are_all_native() {
        let phases = parse("").expect("parses");
        assert!(phases.is_empty());
        assert_eq!(
            phases
                .backing(ReleasePhase::Package, "ecosystems.go.release.phases")
                .expect("resolves"),
            PhaseBacking::Native
        );
        phases
            .validate("ecosystems.go.release.phases")
            .expect("valid");
    }

    #[test]
    fn parses_and_resolves_a_delegated_phase() {
        let phases = parse(
            r#"
            [package]
            backing = "delegated"
            [package.delegated]
            tool = "goreleaser"
            args = ["release", "--clean"]
            "#,
        )
        .expect("parses");

        phases
            .validate("ecosystems.go.release.phases")
            .expect("valid");
        assert_eq!(
            phases
                .backing(ReleasePhase::Package, "ecosystems.go.release.phases")
                .expect("resolves"),
            PhaseBacking::delegated("goreleaser")
        );
        // An unconfigured phase stays native.
        assert_eq!(
            phases
                .backing(ReleasePhase::Tag, "ecosystems.go.release.phases")
                .expect("resolves"),
            PhaseBacking::Native
        );
    }

    #[test]
    fn resolve_rejects_an_inconsistent_entry_instead_of_defaulting() {
        // `backing = "delegated"` with no tool sub-block must not silently
        // resolve to a native backing.
        let phases = parse(
            r#"
            [package]
            backing = "delegated"
            "#,
        )
        .expect("parses");
        let error = phases
            .backing(ReleasePhase::Package, "ecosystems.go.release.phases")
            .expect_err("inconsistent entry rejected on resolve");
        assert!(error.to_string().contains("package.delegated"), "{error}");
    }

    #[test]
    fn native_is_the_default_backing_kind() {
        let phases = parse(
            r"
            [package]
            ",
        )
        .expect("parses");
        assert_eq!(
            phases.0[&ReleasePhase::Package].backing,
            PhaseBackingKind::Native
        );
        phases
            .validate("ecosystems.go.release.phases")
            .expect("valid");
    }

    #[test]
    fn rejects_unknown_phase_name() {
        let error = parse(
            r"
            [packaging]
            ",
        )
        .expect_err("unknown phase rejected");
        assert!(error.to_string().contains("packaging"), "{error}");
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(
            parse(
                r"
            [package]
            bogus = true
            "
            )
            .is_err()
        );
    }

    #[test]
    fn validate_rejects_delegated_without_tool_block() {
        let phases = parse(
            r#"
            [package]
            backing = "delegated"
            "#,
        )
        .expect("parses");
        let error = phases
            .validate("ecosystems.go.release.phases")
            .expect_err("delegated without tool rejected");
        assert!(error.to_string().contains("package.delegated"), "{error}");
    }

    #[test]
    fn validate_rejects_tool_block_without_delegated_backing() {
        let phases = parse(
            r#"
            [package.delegated]
            tool = "goreleaser"
            "#,
        )
        .expect("parses");
        let error = phases
            .validate("ecosystems.go.release.phases")
            .expect_err("native backing with tool rejected");
        assert!(error.to_string().contains("delegated"), "{error}");
    }

    #[test]
    fn validate_rejects_blank_tool() {
        let phases = parse(
            r#"
            [package]
            backing = "delegated"
            [package.delegated]
            tool = "  "
            "#,
        )
        .expect("parses");
        let error = phases
            .validate("ecosystems.go.release.phases")
            .expect_err("blank tool rejected");
        assert!(error.to_string().contains("tool"), "{error}");
    }

    #[test]
    fn round_trips_through_serde() {
        let phases = parse(
            r#"
            [package]
            backing = "delegated"
            [package.delegated]
            tool = "goreleaser"
            "#,
        )
        .expect("parses");
        let toml = toml::to_string(&phases).expect("serializes");
        let back = parse(&toml).expect("round-trips");
        assert_eq!(back, phases);
    }
}
