//! Publishing to a cargo registry: the `cargo publish` argv, the registry
//! token mapping, and classifying the outcome (including crates.io's rate
//! limit).

use std::path::Path;
use std::time::{Duration, SystemTime};

use rskit_errors::{AppError, AppResult};
use rskit_util::time::parse_rfc2822;
use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{
    Artifact, ForwardEnvAs, PublishOutcome, Publisher, RegistryCadence, ReleaseCredentials,
    ToolOutcome, VersionSource, Visibility,
};

use super::target::{CargoRegistryTarget, MAX_CARGO_OUTPUT_BYTES};

/// The environment variable cargo reads for the registry publish token. A
/// configured `token_env` is resolved to its value and injected under this name
/// on the `cargo publish` child, so the secret never appears on argv.
const CARGO_REGISTRY_TOKEN_ENV: &str = "CARGO_REGISTRY_TOKEN";

/// crates.io's refill interval for publishing a brand-new crate name.
const NEW_CRATE_REFILL: Duration = Duration::from_mins(10);

/// crates.io's refill interval for publishing a new version of an existing
/// crate.
const NEW_VERSION_REFILL: Duration = Duration::from_mins(1);

impl Publisher for CargoRegistryTarget {
    fn publish(
        &self,
        module: &Module,
        _artifact: &Artifact,
        credentials: &ReleaseCredentials,
        visibility: Visibility,
    ) -> AppResult<PublishOutcome> {
        let registry = credentials.registry();
        // crates.io (the cargo default) is a public-only registry: every
        // published version is world readable. Fail closed rather than publish a
        // version a maintainer asked to keep private/internal to a registry that
        // cannot honor it. A named alternate registry is assumed to support the
        // requested exposure (its own access controls define it), so the publish
        // proceeds to that registry.
        if is_default_registry(registry) && !visibility.is_public() {
            return Err(AppError::invalid_input(
                "release.visibility",
                format!(
                    "crates.io only publishes public versions, but module '{}' requests \
                     visibility = {}; publish it to a registry that supports that exposure or set \
                     visibility = public",
                    module.key(),
                    visibility.as_str(),
                ),
            ));
        }
        let path = Self::manifest_path(module)?;
        // Resolve the registry-token *mapping* here, at the toolchain boundary —
        // which ambient variable holds the token and which name cargo reads it
        // under — and hand only those names across the seam. The runner resolves
        // the value at run time, so it never touches argv or engine memory.
        // `None` lets cargo resolve its ambient credential as usual.
        let token = registry_token_injection(credentials, rskit_util::env::get_non_empty)?;
        let output = self.cargo_with_env(
            Self::working_root()?,
            publish_argv(&path, registry),
            token,
            MAX_CARGO_OUTPUT_BYTES,
        )?;
        classify_publish(self, module, &output)
    }
}

/// Resolve the registry-token environment mapping for a publish attempt.
///
/// Given the publish `credentials` and an environment accessor `env` (a
/// variable name → its non-empty value), return the [`ForwardEnvAs`] mapping the
/// runner uses to forward the token to cargo: the configured ambient
/// `token_env` as the `source`, and the name cargo reads (`CARGO_REGISTRY_TOKEN`
/// for crates.io, or `CARGO_REGISTRIES_<NAME>_TOKEN` for a named alternate
/// registry) as the `child`. Returns `None` when no `token_env` is configured
/// (cargo falls back to its ambient credential). The accessor is consulted only
/// to fail closed on a configured-but-absent/empty variable — the maintainer
/// named an explicit credential source that is not present, so error rather
/// than silently attempt an unauthenticated publish — and the value is
/// **discarded**, never returned, so the secret never crosses the seam by
/// value. The accessor is a parameter so the resolution logic is unit-tested
/// without touching the real process environment.
fn registry_token_injection<F>(
    credentials: &ReleaseCredentials,
    env: F,
) -> AppResult<Option<ForwardEnvAs>>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(name) = credentials.registry_token_env() else {
        return Ok(None);
    };
    if env(name).is_none() {
        return Err(AppError::invalid_input(
            "release.token_env",
            format!(
                "release.token_env names environment variable '{name}', but it is unset or \
                 empty; export the registry token there before publishing"
            ),
        ));
    }
    Ok(Some(ForwardEnvAs::new(
        name,
        cargo_token_env_name(credentials.registry()),
    )))
}

/// Whether `registry` names the cargo default registry (crates.io) rather than
/// a named alternate. `None` and crates.io's canonical names are the default;
/// any other value selects a named alternate registry.
fn is_default_registry(registry: Option<&str>) -> bool {
    matches!(registry, None | Some("crates-io" | "crates.io"))
}

/// Build the `cargo publish` argv for a module, routing to a named alternate
/// registry via `--registry <name>` when one is configured (the cargo default
/// registry, crates.io, adds no flag).
fn publish_argv(manifest: &Path, registry: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "publish".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
        "--allow-dirty".to_string(),
    ];
    if !is_default_registry(registry)
        && let Some(name) = registry
    {
        argv.push("--registry".to_string());
        argv.push(name.to_string());
    }
    argv
}

/// The cargo environment-variable name that carries the publish token for the
/// selected registry: `CARGO_REGISTRY_TOKEN` for the default registry, or
/// `CARGO_REGISTRIES_<NAME>_TOKEN` for a named alternate registry (cargo's
/// config-env convention: the registry name uppercased with every
/// non-alphanumeric byte replaced by `_`).
fn cargo_token_env_name(registry: Option<&str>) -> String {
    match registry {
        Some(name) if !is_default_registry(Some(name)) => {
            format!("CARGO_REGISTRIES_{}_TOKEN", cargo_registry_env_key(name))
        }
        _ => CARGO_REGISTRY_TOKEN_ENV.to_string(),
    }
}

/// Uppercase a registry name and replace every non-alphanumeric character with
/// `_`, matching cargo's `[registries.<name>]` config-env key derivation.
fn cargo_registry_env_key(registry: &str) -> String {
    registry
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn parse_cargo_search_versions(package: &str, stdout: &str) -> Vec<Version> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once('=')?;
            if name.trim() != package {
                return None;
            }
            let raw = rest.trim().trim_matches('"').split('"').next()?;
            Version::parse(raw).ok()
        })
        .collect()
}

fn classify_publish(
    target: &CargoRegistryTarget,
    module: &Module,
    output: &ToolOutcome,
) -> AppResult<PublishOutcome> {
    if output.succeeded() {
        return Ok(PublishOutcome::Published);
    }

    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    if combined.contains("already uploaded")
        || combined.contains("already exists")
        || combined.contains("already been uploaded")
    {
        return Ok(PublishOutcome::AlreadyPublished);
    }

    if combined.contains("429")
        || combined.contains("rate limit")
        || combined.contains("too many requests")
    {
        // crates.io applies a stricter cadence to a brand-new crate name than to a new
        // version of an existing one. Treat "no versions on the registry" as a first
        // publish; a failed lookup falls back to the existing-name cadence.
        let is_new_release = target
            .published_versions(module)
            .is_ok_and(|versions| versions.is_empty());
        let now = SystemTime::now();
        // Prefer crates.io's own authoritative deadline ("try again after <HTTP-date>")
        // when it parses to a future instant; only guess the cadence when the rejection
        // carries no usable hint (or the hint is already in the past from clock skew).
        let retry_after =
            retry_after_hint(&combined, now).or_else(|| fallback_retry_after(is_new_release, now));
        return Ok(PublishOutcome::RateLimited { retry_after });
    }

    output
        .require_success("publish tool `cargo`")
        .map(|()| PublishOutcome::Published)
}

/// Extract crates.io's rate-limit deadline from a rejected `cargo publish`
/// output. crates.io renders it as `...try again after <RFC 2822 date> and
/// see...`, where the date is terminated by its `GMT` zone. Returns the instant
/// only when it parses and lies in the future — a past or absent hint yields
/// `None` so the caller falls back to the cadence rather than busy-retrying.
fn retry_after_hint(output: &str, now: SystemTime) -> Option<SystemTime> {
    // `output` is already lowercased by the caller; `parse_rfc2822` is
    // case-insensitive, and the marker/zone matching below assumes lowercase.
    let tail = output
        .split_once("try again after ")
        .map(|(_, rest)| rest)?;
    // Cut at the inclusive `gmt` zone token that closes the date.
    let end = tail.find("gmt")? + "gmt".len();
    let epoch_secs = parse_rfc2822(tail.get(..end)?)?;
    let deadline =
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(epoch_secs).ok()?))?;
    (deadline > now).then_some(deadline)
}

fn fallback_retry_after(is_new_release: bool, now: SystemTime) -> Option<SystemTime> {
    RegistryCadence::new(NEW_CRATE_REFILL, NEW_VERSION_REFILL)
        .fallback_retry_after(is_new_release, now)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{ForwardEnvAs, ReleaseCredentials};
    use toven_testkit::doubles::FakeToolRunner;

    use super::{
        CargoRegistryTarget, cargo_token_env_name, fallback_retry_after,
        parse_cargo_search_versions, publish_argv, registry_token_injection, retry_after_hint,
    };

    fn target() -> CargoRegistryTarget {
        CargoRegistryTarget::new(Arc::new(FakeToolRunner::new()))
    }

    fn module() -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("ecosystem"), "core").expect("module"),
            RepoPath::new("crates/core").expect("path"),
        )
    }

    #[test]
    fn parses_matching_crate_version_from_cargo_search() {
        // Representative `cargo search <name> --limit 1` stdout: one `name = "ver"`
        // line per crate, optionally trailed by a `# description` comment.
        let stdout = "\
core = \"1.4.2\"    # A core crate
";
        assert_eq!(
            parse_cargo_search_versions("core", stdout),
            vec![Version::new(1, 4, 2)]
        );
    }

    #[test]
    fn retry_after_hint_honors_a_future_crates_io_deadline() {
        // 2100-01-01T00:00:00Z is 4_102_444_800 epoch seconds.
        const DEADLINE_EPOCH_SECS: u64 = 4_102_444_800;
        // The exact rate-limit rejection crates.io emits, lowercased as the
        // classifier sees it. The deadline sits far in the future so the hint
        // must be preferred over any cadence fallback.
        let output = "error: failed to publish to registry at https://crates.io\n\
            caused by: the remote server responded with an error (status 429 too many requests): \
            you have published too many new crates in a short period of time. \
            please try again after fri, 01 jan 2100 00:00:00 gmt and see \
            https://crates.io/docs/rate-limits for more details.";
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let deadline = retry_after_hint(output, now).expect("a future GMT deadline parses");
        assert_eq!(
            deadline,
            SystemTime::UNIX_EPOCH + Duration::from_secs(DEADLINE_EPOCH_SECS)
        );
    }

    #[test]
    fn retry_after_hint_ignores_a_past_deadline() {
        // A deadline already in the past (clock skew, or a stale message) must
        // not be honored — the caller falls back to the cadence instead.
        let output = "status 429 too many requests. please try again after \
            sun, 06 nov 1994 08:49:37 gmt and see https://crates.io/docs/rate-limits.";
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(retry_after_hint(output, now), None);
    }

    #[test]
    fn retry_after_hint_absent_without_a_deadline_marker() {
        // A 429 with no "try again after" hint yields nothing to schedule from.
        let output = "status 429 too many requests: slow down.";
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(retry_after_hint(output, now), None);
    }

    #[test]
    fn fallback_cadence_is_longer_for_a_brand_new_crate_name() {
        // crates.io throttles a first-ever crate name (10-minute refill) harder
        // than a new version of an existing name (1-minute refill).
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(
            fallback_retry_after(true, now),
            Some(now + Duration::from_mins(10))
        );
        assert_eq!(
            fallback_retry_after(false, now),
            Some(now + Duration::from_mins(1))
        );
    }

    #[test]
    fn registry_token_injection_maps_a_configured_var_to_cargo_registry_token() {
        // A configured token_env is forwarded to cargo under CARGO_REGISTRY_TOKEN
        // — the name cargo reads — by name only; the value is never carried back
        // across the seam. The accessor confirms presence (fail-closed) but its
        // value is discarded.
        let credentials = ReleaseCredentials::new(Some("MY_REGISTRY_TOKEN".into()), None);
        let injected = registry_token_injection(&credentials, |name| {
            (name == "MY_REGISTRY_TOKEN").then(|| "s3cr3t".to_string())
        })
        .expect("a present token var resolves");
        assert_eq!(
            injected,
            Some(ForwardEnvAs::new(
                "MY_REGISTRY_TOKEN",
                "CARGO_REGISTRY_TOKEN"
            ))
        );
    }

    #[test]
    fn registry_token_injection_targets_a_named_registry_token_var() {
        // A named alternate registry forwards the same configured source var to
        // cargo under CARGO_REGISTRIES_<NAME>_TOKEN — the name cargo reads for
        // that registry — by name only.
        let credentials = ReleaseCredentials::new(Some("CI_TOKEN".into()), Some("my-corp".into()));
        let injected = registry_token_injection(&credentials, |name| {
            (name == "CI_TOKEN").then(|| "s3cr3t".to_string())
        })
        .expect("a present token var resolves");
        assert_eq!(
            injected,
            Some(ForwardEnvAs::new(
                "CI_TOKEN",
                "CARGO_REGISTRIES_MY_CORP_TOKEN"
            ))
        );
    }

    #[test]
    fn registry_token_injection_is_absent_without_a_configured_var() {
        // No token_env means "use cargo's ambient credential" — inject nothing.
        let injected = registry_token_injection(&ReleaseCredentials::default(), |_| {
            panic!("the accessor must not be consulted when no token_env is configured")
        })
        .expect("no token_env resolves to no injection");
        assert_eq!(injected, None);
    }

    #[test]
    fn registry_token_injection_fails_closed_when_the_named_var_is_absent() {
        // A configured-but-absent credential source must fail closed rather than
        // silently attempt an unauthenticated publish.
        let credentials = ReleaseCredentials::new(Some("MISSING_TOKEN".into()), None);
        let error = registry_token_injection(&credentials, |_| None)
            .expect_err("an unset named token var must be a typed error");
        let message = error.to_string();
        assert!(message.contains("release.token_env"), "{message}");
        assert!(message.contains("MISSING_TOKEN"), "{message}");
    }

    #[test]
    fn publish_argv_targets_crates_io_by_default_and_a_named_alternate_registry() {
        let manifest = Path::new("crates/core/Cargo.toml");
        // The cargo default registry (None or crates.io's canonical names) adds
        // no `--registry` flag: crates.io behavior is unchanged.
        for default in [None, Some("crates-io"), Some("crates.io")] {
            let argv = publish_argv(manifest, default);
            assert_eq!(&argv[0..2], &["publish", "--manifest-path"]);
            assert!(argv.iter().all(|arg| arg != "--registry"), "{default:?}");
        }
        // A named alternate registry routes the publish via `--registry <name>`.
        let argv = publish_argv(manifest, Some("my-corp"));
        let idx = argv
            .iter()
            .position(|arg| arg == "--registry")
            .expect("flag");
        assert_eq!(argv[idx + 1], "my-corp");
    }

    #[test]
    fn cargo_token_env_name_follows_cargos_registry_convention() {
        assert_eq!(cargo_token_env_name(None), "CARGO_REGISTRY_TOKEN");
        assert_eq!(
            cargo_token_env_name(Some("crates-io")),
            "CARGO_REGISTRY_TOKEN"
        );
        assert_eq!(
            cargo_token_env_name(Some("my-corp")),
            "CARGO_REGISTRIES_MY_CORP_TOKEN"
        );
    }

    #[test]
    fn cargo_search_parse_ignores_non_matching_and_malformed_lines() {
        let stdout = "\
core-extra = \"9.9.9\"    # different crate, prefix match must not count
not a versions line
core = \"0.2.0\"
";
        // Only the exact-name match is returned; the prefix-similar crate and the
        // malformed line are skipped.
        assert_eq!(
            parse_cargo_search_versions("core", stdout),
            vec![Version::new(0, 2, 0)]
        );
        assert!(parse_cargo_search_versions("absent", stdout).is_empty());
    }

    #[test]
    fn publishing_a_non_public_version_to_crates_io_fails_closed() {
        use toven_ports::{Artifact, Publisher, ReleaseCredentials, Visibility};

        // crates.io publishes every version world-readable, so the adapter is the
        // last line of defense: a private/internal exposure is rejected before
        // any cargo invocation, with a typed, actionable error.
        let artifact = Artifact::new(std::path::PathBuf::from("crates/core"));
        let error = target()
            .publish(
                &module(),
                &artifact,
                &ReleaseCredentials::default(),
                Visibility::Private,
            )
            .expect_err("a private crates.io publish must fail closed");

        assert!(error.to_string().contains("release.visibility"));
        assert!(
            error
                .to_string()
                .contains("crates.io only publishes public")
        );
    }

    #[test]
    fn publishing_a_non_public_version_to_a_named_registry_bypasses_the_crates_io_gate() {
        use toven_ports::{Artifact, Publisher, ReleaseCredentials, Visibility};

        // A named alternate registry is not the public-only crates.io, so the
        // adapter does not reject a private/internal exposure: the publish is
        // allowed to proceed (here it fails later, at manifest resolution — never
        // at the visibility gate).
        let artifact = Artifact::new(std::path::PathBuf::from("crates/core"));
        let error = target()
            .publish(
                &module(),
                &artifact,
                &ReleaseCredentials::new(None, Some("my-corp".into())),
                Visibility::Private,
            )
            .expect_err("no manifest on the fixture module, so publish still fails");

        assert!(
            !error.to_string().contains("release.visibility"),
            "a named registry must not trip the crates.io visibility gate: {error}"
        );
        assert!(error.to_string().contains("manifest"), "{error}");
    }
}
