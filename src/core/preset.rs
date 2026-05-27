//! Command preset contract.

/// Data-driven command template definition.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PresetDefinition {
    /// Preset name.
    pub name: String,
    /// Language this preset targets.
    pub language: String,
    /// Command argv template.
    pub argv: Vec<String>,
    /// Shared input paths that affect every module using this preset.
    pub shared_inputs: Vec<String>,
}
