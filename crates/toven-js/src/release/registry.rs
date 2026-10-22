//! npm registry rules: which registries a release may address, how the token
//! reaches npm, the access level and dist-tag a version publishes with, and
//! how npm's answers are read.

use rskit_codec::{JsonCodec, decode};
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use serde::Deserialize;
use toven_ports::{PublishOutcome, ToolOutcome, Visibility};

/// The `release.registry` value naming the public npm registry.
pub(crate) const NPMJS: &str = "npmjs";

/// The public npm registry.
const NPMJS_URL: &str = "https://registry.npmjs.org/";

/// The dist-tag a stable version publishes under.
pub(crate) const LATEST: &str = "latest";

/// Hosts a plain-`http` registry may use: a local test registry only.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// The registry URL a `release.registry` value addresses, ending in `/`.
///
/// `npmjs` is the public registry. Any other value must be an `https` URL, or
/// an `http` URL on a loopback host (a local test registry), with no
/// credentials, query, or fragment.
///
/// # Errors
/// A typed `release.registry` error for any other value.
pub(crate) fn registry_url(registry: &str) -> AppResult<String> {
    if registry == NPMJS {
        return Ok(NPMJS_URL.to_string());
    }
    let reject = |reason: &str| {
        AppError::invalid_input(
            "release.registry",
            format!(
                "npm registry '{registry}' {reason}; use \"{NPMJS}\" or an https registry URL \
                 (http only for a loopback test registry)"
            ),
        )
    };
    let (secure, rest) = if let Some(rest) = registry.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = registry.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(reject("is not a registry URL"));
    };
    if rest.contains(['?', '#']) || rest.chars().any(char::is_whitespace) {
        return Err(reject("has a query, fragment, or whitespace"));
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.contains('@') {
        return Err(reject(
            "carries credentials; pass the token through release.token_env",
        ));
    }
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err(reject("has no host"));
    }
    if !secure && !LOOPBACK_HOSTS.contains(&host) {
        return Err(reject("uses plain http on a non-loopback host"));
    }
    let mut url = registry.to_string();
    if !url.ends_with('/') {
        url.push('/');
    }
    Ok(url)
}

/// The npm config environment variable that carries the auth token for
/// `url`: `npm_config_//host/path/:_authToken`. npm reads per-registry
/// config from the environment under this name, so the token never reaches
/// argv or an `.npmrc` file.
pub(crate) fn token_env_name(url: &str) -> String {
    let nerf = url
        .strip_prefix("https:")
        .or_else(|| url.strip_prefix("http:"))
        .unwrap_or(url);
    format!("npm_config_{nerf}:_authToken")
}

/// The `--access` value for `visibility`.
///
/// npm has public and restricted (private) packages, and only scoped packages
/// can be restricted. It has no organization-internal exposure.
///
/// # Errors
/// A typed `release.visibility` error for `internal`, or for `private` on an
/// unscoped package.
pub(crate) fn access(package: &str, visibility: Visibility) -> AppResult<&'static str> {
    match visibility {
        Visibility::Public => Ok("public"),
        Visibility::Private if package.starts_with('@') => Ok("restricted"),
        Visibility::Private => Err(AppError::invalid_input(
            "release.visibility",
            format!(
                "npm package '{package}' is unscoped, and only scoped packages can be private; \
                 scope it (@scope/name) or set visibility = public"
            ),
        )),
        _ => Err(AppError::invalid_input(
            "release.visibility",
            format!(
                "npm package '{package}' requests visibility = {}, but npm has only public and \
                 private (restricted) packages",
                visibility.as_str()
            ),
        )),
    }
}

/// The dist-tag `version` publishes under: `latest` for a stable version, and
/// the prerelease channel (the first prerelease identifier, `rc` in
/// `1.0.0-rc.1`) otherwise, so a prerelease never becomes what a plain
/// install resolves.
///
/// # Errors
/// A typed `release.version` error for a prerelease whose first identifier is
/// numeric or `latest`, since neither names a channel.
pub(crate) fn dist_tag(package: &str, version: &Version) -> AppResult<String> {
    if version.pre.is_empty() {
        return Ok(LATEST.to_string());
    }
    let channel = version.pre.as_str().split('.').next().unwrap_or_default();
    if channel.is_empty() || channel == LATEST || channel.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(AppError::invalid_input(
            "release.version",
            format!(
                "prerelease {package}@{version} has no channel to publish under: its first \
                 prerelease identifier must name one (for example 1.0.0-rc.1), and it must not be \
                 '{LATEST}' or a number, so the prerelease never becomes '{LATEST}'"
            ),
        ));
    }
    Ok(channel.to_string())
}

/// npm's JSON answer to `npm view <name> versions --json`: one version is a
/// bare string, several are an array, and a failure is an error object.
#[derive(Deserialize)]
#[serde(untagged)]
enum ViewAnswer {
    One(String),
    Many(Vec<String>),
    Failed { error: ViewError },
}

/// The error object npm prints for a failed `npm view --json`.
#[derive(Deserialize)]
struct ViewError {
    code: Option<String>,
}

/// The versions an `npm view <name> versions --json` run reports. A package
/// the registry has never seen (`E404`) has none.
///
/// # Errors
/// A timeout, cancellation, or any other failure as a typed error, and
/// [`ErrorCode::InvalidFormat`] for output that is not npm's JSON.
pub(crate) fn published_versions(package: &str, output: &ToolOutcome) -> AppResult<Vec<Version>> {
    let context = "registry query tool `npm`";
    if output.timed_out || output.cancelled || output.truncated.any() {
        output.require_success(context)?;
    }
    let stdout = output.stdout.trim();
    let answer = if stdout.is_empty() {
        None
    } else {
        Some(
            decode::<ViewAnswer>(&JsonCodec::default(), stdout).map_err(|error| {
                AppError::new(
                    ErrorCode::InvalidFormat,
                    format!("`npm view {package} versions --json` printed output that is not JSON"),
                )
                .with_cause(error)
            })?,
        )
    };
    match answer {
        Some(ViewAnswer::Failed { error }) if error.code.as_deref() == Some("E404") => {
            Ok(Vec::new())
        }
        Some(ViewAnswer::One(raw)) if output.succeeded() => Ok(parse_versions(&[raw])),
        Some(ViewAnswer::Many(raw)) if output.succeeded() => Ok(parse_versions(&raw)),
        None if output.succeeded() => Ok(Vec::new()),
        _ => {
            output.require_success(context)?;
            Err(AppError::new(
                ErrorCode::ExternalService,
                format!("`npm view {package} versions --json` reported an error"),
            ))
        }
    }
}

/// The parseable versions among `raw`; a registry may hold legacy versions
/// that are not strict semver, and those can never collide with a release.
fn parse_versions(raw: &[String]) -> Vec<Version> {
    raw.iter()
        .filter_map(|version| Version::parse(version).ok())
        .collect()
}

/// Classify one `npm publish` attempt.
///
/// # Errors
/// [`ErrorCode::Unauthorized`] or [`ErrorCode::Forbidden`] when the registry
/// refuses the credential, and the shared typed error for a timeout,
/// cancellation, or any other failure.
pub(crate) fn classify_publish(package: &str, output: &ToolOutcome) -> AppResult<PublishOutcome> {
    if output.succeeded() {
        return Ok(PublishOutcome::Published);
    }
    let context = "publish tool `npm`";
    if output.timed_out || output.cancelled {
        output.require_success(context)?;
    }
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    if combined.contains("cannot publish over")
        || combined.contains("previously published")
        || combined.contains("code e409")
    {
        return Ok(PublishOutcome::AlreadyPublished);
    }
    if combined.contains("code e429") || combined.contains("too many requests") {
        return Ok(PublishOutcome::RateLimited { retry_after: None });
    }
    if combined.contains("code e401") || combined.contains("code eneedauth") {
        return Err(AppError::new(
            ErrorCode::Unauthorized,
            format!(
                "the registry rejected the credential publishing '{package}'; set \
                 release.token_env to a variable holding a valid token, or use trusted publishing"
            ),
        ));
    }
    if combined.contains("code e403") {
        return Err(AppError::new(
            ErrorCode::Forbidden,
            format!(
                "the registry refused to publish '{package}': the credential cannot publish this \
                 package, or the version is not allowed"
            ),
        ));
    }
    output.require_success(context)?;
    Ok(PublishOutcome::Published)
}

#[cfg(test)]
mod tests {
    use rskit_errors::ErrorCode;
    use rskit_version::semver::Version;
    use toven_ports::{PublishOutcome, ToolOutcome, Visibility};

    use super::{
        access, classify_publish, dist_tag, published_versions, registry_url, token_env_name,
    };

    fn outcome(code: i32, stdout: &str, stderr: &str) -> ToolOutcome {
        ToolOutcome::new(Some(code), stdout, stderr)
    }

    #[test]
    fn registry_values_resolve_to_urls() {
        assert_eq!(
            registry_url("npmjs").unwrap(),
            "https://registry.npmjs.org/"
        );
        assert_eq!(
            registry_url("https://npm.pkg.github.com").unwrap(),
            "https://npm.pkg.github.com/"
        );
        assert_eq!(
            registry_url("http://localhost:4873/").unwrap(),
            "http://localhost:4873/"
        );
        assert_eq!(
            registry_url("http://[::1]:4873").unwrap(),
            "http://[::1]:4873/"
        );
    }

    #[test]
    fn unsafe_or_unknown_registries_are_rejected() {
        for registry in [
            "crates-io",
            "http://registry.example.com/",
            "https://user:pass@registry.example.com/",
            "https://registry.example.com/?token=x",
            "https://",
        ] {
            let error = registry_url(registry).unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidInput, "{registry}");
            assert!(error.to_string().contains("release.registry"), "{error}");
        }
    }

    #[test]
    fn the_token_travels_under_the_registry_config_key() {
        assert_eq!(
            token_env_name("https://registry.npmjs.org/"),
            "npm_config_//registry.npmjs.org/:_authToken"
        );
        assert_eq!(
            token_env_name("http://localhost:4873/npm/"),
            "npm_config_//localhost:4873/npm/:_authToken"
        );
    }

    #[test]
    fn visibility_maps_to_access() {
        assert_eq!(access("@a/b", Visibility::Public).unwrap(), "public");
        assert_eq!(access("@a/b", Visibility::Private).unwrap(), "restricted");
        for (package, visibility) in [("b", Visibility::Private), ("@a/b", Visibility::Internal)] {
            let error = access(package, visibility).unwrap_err();
            assert!(error.to_string().contains("release.visibility"), "{error}");
        }
    }

    #[test]
    fn prereleases_publish_under_their_channel_never_latest() {
        let tag = |raw: &str| dist_tag("p", &Version::parse(raw).unwrap());
        assert_eq!(tag("1.0.0").unwrap(), "latest");
        assert_eq!(tag("1.0.0-rc.1").unwrap(), "rc");
        assert_eq!(tag("1.0.0-beta").unwrap(), "beta");
        for raw in ["1.0.0-0", "1.0.0-latest.1"] {
            let error = tag(raw).unwrap_err();
            assert!(
                error.to_string().contains("never becomes 'latest'"),
                "{error}"
            );
        }
    }

    #[test]
    fn published_versions_read_every_npm_answer_shape() {
        let versions = |output: ToolOutcome| {
            published_versions("p", &output)
                .unwrap()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            versions(outcome(0, r#"["1.0.0","1.1.0-rc.1","bogus"]"#, "")),
            ["1.0.0", "1.1.0-rc.1"]
        );
        assert_eq!(versions(outcome(0, "\"1.0.0\"\n", "")), ["1.0.0"]);
        assert!(versions(outcome(1, r#"{"error":{"code":"E404","summary":"x"}}"#, "")).is_empty());
    }

    #[test]
    fn published_versions_fail_closed() {
        let timed_out = outcome(0, "", "").timed_out_flag(true);
        assert_eq!(
            published_versions("p", &timed_out).unwrap_err().code(),
            ErrorCode::Timeout
        );
        let denied = outcome(1, r#"{"error":{"code":"E401"}}"#, "");
        assert_eq!(
            published_versions("p", &denied).unwrap_err().code(),
            ErrorCode::ExternalService
        );
        let garbage = outcome(0, "not json", "");
        assert_eq!(
            published_versions("p", &garbage).unwrap_err().code(),
            ErrorCode::InvalidFormat
        );
    }

    #[test]
    fn publish_answers_are_classified() {
        assert_eq!(
            classify_publish("p", &outcome(0, "+ p@1.0.0", "")).unwrap(),
            PublishOutcome::Published
        );
        let over = "npm error code E403\nnpm error 403 Forbidden - PUT https://r/p - You cannot \
                    publish over the previously published versions: 1.0.0.";
        assert_eq!(
            classify_publish("p", &outcome(1, "", over)).unwrap(),
            PublishOutcome::AlreadyPublished
        );
        assert_eq!(
            classify_publish("p", &outcome(1, "", "npm error code E429")).unwrap(),
            PublishOutcome::RateLimited { retry_after: None }
        );
        for (stderr, code) in [
            ("npm error code E401", ErrorCode::Unauthorized),
            ("npm error code ENEEDAUTH", ErrorCode::Unauthorized),
            (
                "npm error code E403\nnpm error 403 Forbidden",
                ErrorCode::Forbidden,
            ),
            ("npm error code E500", ErrorCode::ExternalService),
        ] {
            assert_eq!(
                classify_publish("p", &outcome(1, "", stderr))
                    .unwrap_err()
                    .code(),
                code,
                "{stderr}"
            );
        }
        let timed_out = outcome(0, "", "code E401").timed_out_flag(true);
        assert_eq!(
            classify_publish("p", &timed_out).unwrap_err().code(),
            ErrorCode::Timeout
        );
    }
}
