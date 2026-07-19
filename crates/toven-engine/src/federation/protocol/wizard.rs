//! The config-less wizard exchange — the federated half of `toven init`.
//!
//! Unlike the [`envelope`](super::envelope) RPC mirror (which configures an
//! adapter from an `[ecosystems.<id>]` subtree before answering port calls),
//! the wizard runs **before any config exists**. So it is a distinct,
//! **two-round-trip** exchange over the same length-delimited
//! [`codec`](super::codec) framing:
//!
//! 1. The umbrella sends a [`WizardProbe`] naming the project root; the driven
//!    `toven-<eco> __init` process self-detects every ecosystem it serves and
//!    answers with a [`WizardOffer`] carrying each [`Detection`] and its
//!    [`Questionnaire`]. **The driver stays alive**, holding its detections.
//! 2. The umbrella prompts the offered questionnaires (locally), then sends a
//!    [`WizardAnswers`] carrying the per-ecosystem [`Answers`]. The driver
//!    re-associates each answer set with its stored detection, calls `render`,
//!    and replies with a [`WizardResult`] of [`EcosystemFragment`]s (or a typed
//!    error), then exits.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use toven_model::EcosystemId;
use toven_ports::{Answers, Detection, EcosystemFragment, Questionnaire};

use super::envelope::{ENVELOPE_SCHEMA_VERSION, WireError};

/// The umbrella's config-less wizard probe: "what do you detect under here?".
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct WizardProbe {
    /// Envelope schema version ([`ENVELOPE_SCHEMA_VERSION`]).
    pub schema_version: u16,
    /// The project root the driver self-detects its ecosystems under.
    pub project_root: PathBuf,
}

impl WizardProbe {
    /// Build a wizard probe for `project_root` at the current schema version.
    #[must_use]
    pub const fn new(project_root: PathBuf) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            project_root,
        }
    }
}

/// One detected ecosystem the driver offers for onboarding: its [`Detection`]
/// and the [`Questionnaire`] the umbrella must answer before render.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct WizardOffering {
    /// The adapter's probe result, carried back unchanged after answering.
    pub detection: Detection,
    /// The declarative questionnaire the umbrella prompts for this ecosystem.
    pub questionnaire: Questionnaire,
}

/// A driven `__init` process's first reply: every detected ecosystem (each with
/// its questionnaire), or a typed failure.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum WizardOffer {
    /// The detected ecosystems (possibly empty when the driver serves none
    /// under the root), each with the questionnaire to answer before render.
    Detected(Vec<WizardOffering>),
    /// The driver could not complete detection; carries a typed cause.
    Error(WireError),
}

/// One ecosystem's resolved answers, keyed so the driver re-associates them
/// with the matching stored [`Detection`] before rendering.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WizardAnswerEntry {
    /// The ecosystem the answers are for (matches an offered detection).
    pub ecosystem: EcosystemId,
    /// The umbrella's resolved answers to that ecosystem's questionnaire.
    pub answers: Answers,
}

/// The umbrella's second message: the per-ecosystem answers to render from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WizardAnswers {
    /// Envelope schema version ([`ENVELOPE_SCHEMA_VERSION`]).
    pub schema_version: u16,
    /// The resolved answers, one entry per offered ecosystem.
    pub entries: Vec<WizardAnswerEntry>,
}

impl WizardAnswers {
    /// Build a wizard-answers message at the current schema version.
    #[must_use]
    pub const fn new(entries: Vec<WizardAnswerEntry>) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            entries,
        }
    }
}

/// A driven `__init` process's second reply: the rendered fragments, or a
/// failure.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum WizardResult {
    /// Every `[ecosystems.<id>]` fragment the driver rendered from the answers.
    Fragments(Vec<EcosystemFragment>),
    /// The driver could not render; carries a typed, displayable cause.
    Error(WireError),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use toml::Table;
    use toven_ports::{Detection, EcosystemFragment, Questionnaire};

    use super::super::envelope::WireError;
    use super::{
        WizardAnswerEntry, WizardAnswers, WizardOffer, WizardOffering, WizardProbe, WizardResult,
    };

    fn rust() -> toven_model::EcosystemId {
        toven_model::EcosystemId::new("rust").expect("valid id")
    }

    #[test]
    fn probe_round_trips_through_json() {
        let probe = WizardProbe::new(PathBuf::from("/repo"));
        let json = serde_json::to_string(&probe).expect("serialize");
        let back: WizardProbe = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(probe, back);
    }

    #[test]
    fn offer_round_trips_through_json() {
        let offering = WizardOffering {
            detection: Detection::bare(rust()),
            questionnaire: Questionnaire::empty(rust()),
        };
        let offer = WizardOffer::Detected(vec![offering]);
        let json = serde_json::to_string(&offer).expect("serialize");
        let back: WizardOffer = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(offer, back);
    }

    #[test]
    fn answers_round_trip_through_json() {
        let answers = WizardAnswers::new(vec![WizardAnswerEntry {
            ecosystem: rust(),
            answers: toven_ports::Answers::new(),
        }]);
        let json = serde_json::to_string(&answers).expect("serialize");
        let back: WizardAnswers = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(answers, back);
    }

    #[test]
    fn result_round_trips_through_json() {
        let result = WizardResult::Fragments(vec![EcosystemFragment::new(rust(), Table::new())]);
        let json = serde_json::to_string(&result).expect("serialize");
        let back: WizardResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, back);
    }

    #[test]
    fn error_offer_round_trips_through_json() {
        let offer = WizardOffer::Error(WireError::new(
            rskit_errors::ErrorCode::Internal.as_str(),
            "boom",
        ));
        let json = serde_json::to_string(&offer).expect("serialize");
        let back: WizardOffer = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(offer, back);
    }
}
