//! Shared onboarding questions that author an opt-in `[…release]` block.
//!
//! Release automation is opt-in: the wizard asks a single confirm first, and
//! only when the user says yes does it ask the follow-ups and author a
//! `[…release]` block. Declining leaves the ecosystem release-free — the
//! returned [`ReleaseConfig`] is [default](ReleaseConfig::is_default), so the
//! renderer skips it entirely.
//!
//! Both the Rust and Go adapters reuse this builder so the two ecosystems ask
//! the same release questions and resolve them identically; the only ecosystem
//! difference is the registry. A registry-publishable ecosystem (Rust →
//! crates.io) passes its registry id and gets a registry question; a tag-only
//! ecosystem (Go module tags) passes `None`, which omits the registry question
//! and leaves the release tag-only.

use rskit_cli::Choice;

use crate::config::{HostConfig, PrereleaseConfig, ReleaseConfig};

use super::{Answers, Question, QuestionKind};

/// Question id for the opt-in release confirm.
pub const RELEASE_ENABLED: &str = "release-enabled";

/// Question id for the publish-registry selection (registry ecosystems only).
pub const RELEASE_REGISTRY: &str = "release-registry";

/// Question id for the prerelease-channel multi-select.
pub const RELEASE_PRERELEASE: &str = "release-prerelease";

/// Question id for the hosted-Release confirm.
pub const RELEASE_HOST: &str = "release-host";

/// Choice id opting out of a publish registry (a tag-only release).
pub const REGISTRY_NONE: &str = "none";

/// The prerelease channels offered by the multi-select, in menu order.
const PRERELEASE_CHANNELS: [&str; 3] = ["alpha", "beta", "rc"];

/// The forge the hosted-Release confirm authors when accepted.
const HOSTED_FORGE: &str = "github";

/// The clean-tree readiness check, authored for every opted-in release.
const CHECK_CLEAN_TREE: &str = "clean-tree";

/// The registry-idempotency readiness check, authored when a registry is set.
const CHECK_REGISTRY_IDEMPOTENT: &str = "registry-idempotent";

/// Build the opt-in release questions for an ecosystem.
///
/// `registry` is the ecosystem's default publish registry (e.g. `"crates-io"`
/// for Rust); pass `None` for a tag-only ecosystem (Go module tags), which
/// omits the registry question. The first question is a confirm defaulted to
/// `false`, so a non-interactive run never opts a repository into releases it
/// did not ask for.
#[must_use]
pub fn release_questions(registry: Option<&str>) -> Vec<Question> {
    let mut questions = vec![Question::new(
        RELEASE_ENABLED,
        "Configure release automation for this ecosystem?",
        QuestionKind::Confirm { default: false },
    )];

    if let Some(registry) = registry {
        questions.push(
            Question::new(
                RELEASE_REGISTRY,
                "Which registry do releasable modules publish to?",
                QuestionKind::Select(vec![
                    Choice::new(registry, registry).recommended(),
                    Choice::new(REGISTRY_NONE, "no registry (tag-only)"),
                ]),
            )
            .asked_when(RELEASE_ENABLED),
        );
    }

    questions.push(
        Question::new(
            RELEASE_PRERELEASE,
            "Which prerelease channels should releases support? (none = stable-only)",
            QuestionKind::MultiSelect(
                PRERELEASE_CHANNELS
                    .iter()
                    .map(|channel| Choice::new(*channel, *channel))
                    .collect(),
            ),
        )
        .asked_when(RELEASE_ENABLED),
    );

    questions.push(
        Question::new(
            RELEASE_HOST,
            "Cut a hosted GitHub Release after publishing?",
            QuestionKind::Confirm { default: false },
        )
        .asked_when(RELEASE_ENABLED),
    );

    questions
}

/// Resolve the release answers into a [`ReleaseConfig`].
///
/// Returns a default config (so the renderer skips the block) unless the user
/// opted in via [`RELEASE_ENABLED`]. When opted in it folds the answers into a
/// minimal, valid block: the chosen registry (or none for tag-only), the
/// selected prerelease channels, an opt-in `github` hosted Release, and a
/// readiness gate (`clean-tree`, plus `registry-idempotent` when a registry is
/// set). `registry` is the same value passed to [`release_questions`].
#[must_use]
pub fn release_config(answers: &Answers, registry: Option<&str>) -> ReleaseConfig {
    if answers.bool(&RELEASE_ENABLED.into()) != Some(true) {
        return ReleaseConfig::default();
    }

    let registry = resolve_registry(answers, registry);
    let mut config = ReleaseConfig {
        registry: registry.clone(),
        ..ReleaseConfig::default()
    };

    let channels = selected_channels(answers);
    if !channels.is_empty() {
        config.prerelease = Some(PrereleaseConfig {
            channels,
            ..PrereleaseConfig::default()
        });
    }

    if answers.bool(&RELEASE_HOST.into()) == Some(true) {
        config.host = Some(HostConfig {
            forge: Some(HOSTED_FORGE.to_string()),
            ..HostConfig::default()
        });
    }

    config.readiness = Some(readiness(registry.is_some()));
    config
}

/// Resolve the publish registry: honor the selection for a registry ecosystem,
/// treating [`REGISTRY_NONE`] (or a tag-only ecosystem) as no registry.
fn resolve_registry(answers: &Answers, registry: Option<&str>) -> Option<String> {
    let default = registry?;
    match answers
        .choice(&RELEASE_REGISTRY.into())
        .map(rskit_cli::ChoiceId::as_str)
    {
        Some(REGISTRY_NONE) => None,
        Some(selected) => Some(selected.to_string()),
        None => Some(default.to_string()),
    }
}

/// The prerelease channels the user selected, in menu order.
fn selected_channels(answers: &Answers) -> Vec<String> {
    let Some(selected) = answers.multi_choice(&RELEASE_PRERELEASE.into()) else {
        return Vec::new();
    };
    PRERELEASE_CHANNELS
        .iter()
        .filter(|channel| selected.iter().any(|id| id.as_str() == **channel))
        .map(|channel| (*channel).to_string())
        .collect()
}

/// The readiness checks authored for an opted-in release.
fn readiness(has_registry: bool) -> Vec<String> {
    let mut checks = vec![CHECK_CLEAN_TREE.to_string()];
    if has_registry {
        checks.push(CHECK_REGISTRY_IDEMPOTENT.to_string());
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::{
        REGISTRY_NONE, RELEASE_ENABLED, RELEASE_HOST, RELEASE_PRERELEASE, RELEASE_REGISTRY,
        release_config, release_questions,
    };
    use crate::wizard::{Answer, Answers, QuestionId, QuestionKind};
    use rskit_cli::ChoiceId;

    #[test]
    fn registry_ecosystem_asks_registry_question() {
        let questions = release_questions(Some("crates-io"));
        let ids: Vec<&str> = questions.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                RELEASE_ENABLED,
                RELEASE_REGISTRY,
                RELEASE_PRERELEASE,
                RELEASE_HOST
            ]
        );
    }

    #[test]
    fn tag_only_ecosystem_omits_registry_question() {
        let questions = release_questions(None);
        let ids: Vec<&str> = questions.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, [RELEASE_ENABLED, RELEASE_PRERELEASE, RELEASE_HOST]);
    }

    #[test]
    fn follow_ups_are_gated_on_the_opt_in_confirm() {
        let questions = release_questions(Some("crates-io"));
        for question in &questions {
            let gate = question.ask_if.as_ref().map(QuestionId::as_str);
            if question.id.as_str() == RELEASE_ENABLED {
                assert_eq!(gate, None, "the opt-in confirm itself is never gated");
            } else {
                assert_eq!(
                    gate,
                    Some(RELEASE_ENABLED),
                    "follow-up {} must be gated on the opt-in",
                    question.id
                );
            }
        }
    }

    #[test]
    fn release_confirm_defaults_to_off() {
        let questions = release_questions(Some("crates-io"));
        let QuestionKind::Confirm { default } = questions[0].kind else {
            panic!("expected a confirm question");
        };
        assert!(!default, "release must be opt-in");
    }

    #[test]
    fn declining_release_yields_a_default_skippable_block() {
        let config = release_config(&Answers::new(), Some("crates-io"));
        assert!(config.is_default(), "no block authored when not opted in");
    }

    #[test]
    fn explicit_decline_yields_a_default_block() {
        let answers = Answers::new().with(RELEASE_ENABLED, Answer::Bool(false));
        assert!(release_config(&answers, Some("crates-io")).is_default());
    }

    #[test]
    fn opting_in_authors_the_default_registry_and_readiness() {
        let answers = Answers::new().with(RELEASE_ENABLED, Answer::Bool(true));
        let config = release_config(&answers, Some("crates-io"));

        assert_eq!(config.registry.as_deref(), Some("crates-io"));
        assert_eq!(
            config.readiness.as_deref(),
            Some(["clean-tree".to_string(), "registry-idempotent".to_string()].as_slice())
        );
        assert!(config.prerelease.is_none());
        assert!(config.host.is_none());
        config.validate("ecosystems.rust.release").expect("valid");
    }

    #[test]
    fn opting_out_of_the_registry_is_tag_only() {
        let answers = Answers::new()
            .with(RELEASE_ENABLED, Answer::Bool(true))
            .with(RELEASE_REGISTRY, Answer::Choice(ChoiceId::new(REGISTRY_NONE)));
        let config = release_config(&answers, Some("crates-io"));

        assert!(config.registry.is_none());
        assert_eq!(
            config.readiness.as_deref(),
            Some(["clean-tree".to_string()].as_slice()),
            "registry-idempotent is dropped without a registry",
        );
        config.validate("ecosystems.rust.release").expect("valid");
    }

    #[test]
    fn selected_channels_author_a_prerelease_block_in_menu_order() {
        let answers = Answers::new()
            .with(RELEASE_ENABLED, Answer::Bool(true))
            .with(
                RELEASE_PRERELEASE,
                Answer::MultiChoice(vec![ChoiceId::new("rc"), ChoiceId::new("alpha")]),
            );
        let config = release_config(&answers, None);

        let prerelease = config.prerelease.expect("prerelease authored");
        assert_eq!(prerelease.channels, ["alpha", "rc"], "menu order, not click order");
    }

    #[test]
    fn hosted_confirm_authors_a_github_release() {
        let answers = Answers::new()
            .with(RELEASE_ENABLED, Answer::Bool(true))
            .with(RELEASE_HOST, Answer::Bool(true));
        let config = release_config(&answers, None);

        assert_eq!(
            config.host.as_ref().expect("host authored").forge.as_deref(),
            Some("github")
        );
        config.validate("ecosystems.go.release").expect("valid");
    }

    #[test]
    fn tag_only_ecosystem_never_sets_a_registry() {
        let answers = Answers::new().with(RELEASE_ENABLED, Answer::Bool(true));
        let config = release_config(&answers, None);
        assert!(config.registry.is_none());
        assert_eq!(config.readiness.as_deref(), Some(["clean-tree".to_string()].as_slice()));
    }
}
