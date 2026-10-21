//! The `package.json` edits a release makes: read the version, and write a new
//! version and in-repo dependency ranges without touching any other byte.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;

use crate::discovery::plain_range;
use crate::json_edit::{self, Edit, Node};
use crate::range;

/// The dependency groups whose in-repo ranges a release rewrites.
const DEPENDENCY_GROUPS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// The byte-order mark some editors write; npm accepts it, so edits keep it.
const BOM: char = '\u{feff}';

/// The version `text` (the manifest labelled `label`) declares, or `None` when
/// it declares none.
///
/// # Errors
/// [`ErrorCode::InvalidFormat`] for malformed JSON, a non-object document, a
/// non-string version, or a version that is not semver.
pub(crate) fn version_in(text: &str, label: &str) -> AppResult<Option<Version>> {
    let document = object(text.strip_prefix(BOM).unwrap_or(text), label)?;
    document
        .get("version")
        .map(|member| {
            let (raw, _) = member.value.as_str().ok_or_else(|| non_string(label))?;
            parse_version(raw, label)
        })
        .transpose()
}

/// Rewrite `text` (the manifest labelled `label`) for a release: set its
/// version to `version` when given, and move every plain range on a package
/// named in `floors` to that package's new version.
///
/// A simple range (`^1.2.0`, `~1.2.0`, `>=1.2.0`, `=1.2.0`, `1.2.0`) keeps its
/// operator and takes the new version. `workspace:`, path, alias, Git, and URL
/// specs and dist-tags stay as they are. Any other range is kept when the new
/// version satisfies it, and is a conflict otherwise, because rewriting it
/// would change what the author wrote.
///
/// Returns `None` when nothing changes.
///
/// # Errors
/// [`ErrorCode::InvalidFormat`] for a malformed manifest,
/// [`ErrorCode::InvalidInput`] when a version must be added to a manifest
/// without a `name`, and [`ErrorCode::Conflict`] for a range the new version
/// leaves behind.
pub(crate) fn apply(
    text: &str,
    label: &str,
    version: Option<&Version>,
    floors: &BTreeMap<String, Version>,
) -> AppResult<Option<String>> {
    let (bom, body) = text
        .strip_prefix(BOM)
        .map_or(("", text), |body| (&text[..BOM.len_utf8()], body));
    let document = object(body, label)?;
    let mut edits = Vec::new();
    if let Some(version) = version {
        edits.extend(version_edit(body, &document, label, version)?);
    }
    for group in DEPENDENCY_GROUPS {
        let Some(dependencies) = document
            .get(group)
            .and_then(|member| member.value.members())
        else {
            continue;
        };
        for dependency in dependencies {
            let Some(floor) = floors.get(&dependency.key) else {
                continue;
            };
            let Some((spec, span)) = dependency.value.as_str() else {
                continue;
            };
            if let Some(rewritten) = rewrite_range(&dependency.key, spec, floor, group, label)? {
                edits.push(Edit::string(span, &rewritten));
            }
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("{bom}{}", json_edit::apply(body, edits)?)))
}

/// The edit setting the document's version, or `None` when it already
/// matches. A manifest without a version gets one right after its `name`.
fn version_edit(
    body: &str,
    document: &Node,
    label: &str,
    version: &Version,
) -> AppResult<Option<Edit>> {
    let rendered = version.to_string();
    if let Some(member) = document.get("version") {
        let (current, span) = member.value.as_str().ok_or_else(|| non_string(label))?;
        return Ok((current != rendered).then(|| Edit::string(span, &rendered)));
    }
    let name = document.get("name").ok_or_else(|| {
        AppError::invalid_input(
            "release.version",
            format!("'{label}' has no \"name\", so Toven cannot place a \"version\" after it"),
        )
    })?;
    Ok(Some(json_edit::insert_member_after(
        body, name, "version", &rendered,
    )))
}

/// The rewritten spec for the dependency `name` declared with `spec` once it
/// releases `floor`, or `None` to keep `spec`.
fn rewrite_range(
    name: &str,
    spec: &str,
    floor: &Version,
    group: &str,
    label: &str,
) -> AppResult<Option<String>> {
    let Some(range) = plain_range(name, spec) else {
        return Ok(None);
    };
    if let Some(operator) = simple_operator(range) {
        let rewritten = format!("{operator}{floor}");
        return Ok((rewritten != spec).then_some(rewritten));
    }
    if !range::is_range(range) || range::satisfies(&floor.to_string(), range) {
        return Ok(None);
    }
    Err(AppError::new(
        ErrorCode::Conflict,
        format!(
            "'{label}' {group} '{name}' is '{spec}', which does not allow the released version \
             {floor}; widen the range or change it to a simple range (for example '^{floor}') \
             that Toven can move"
        ),
    ))
}

/// The operator of a simple range — one operator Toven can keep, then one
/// full semver version — or `None` for any other range.
fn simple_operator(range: &str) -> Option<&str> {
    let split = range
        .find(|ch: char| !matches!(ch, '^' | '~' | '>' | '='))
        .unwrap_or(range.len());
    let (operator, version) = range.split_at(split);
    let version = version.strip_prefix(['v', 'V']).unwrap_or(version);
    (matches!(operator, "" | "^" | "~" | ">=" | "=") && Version::parse(version).is_ok())
        .then_some(operator)
}

/// Parse `body` as a JSON object.
fn object(body: &str, label: &str) -> AppResult<Node> {
    let document = json_edit::parse(body).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("'{label}' is not valid JSON"),
        )
        .with_cause(error)
    })?;
    if document.members().is_none() {
        return Err(AppError::new(
            ErrorCode::InvalidFormat,
            format!("'{label}' is not a JSON object"),
        ));
    }
    Ok(document)
}

/// The error for a `version` that is not a string.
fn non_string(label: &str) -> AppError {
    AppError::new(
        ErrorCode::InvalidFormat,
        format!("'{label}' declares a non-string \"version\""),
    )
}

/// Parse a declared version.
fn parse_version(raw: &str, label: &str) -> AppResult<Version> {
    Version::parse(raw).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("'{label}' declares version '{raw}', which is not a semantic version"),
        )
        .with_cause(error)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_errors::ErrorCode;
    use rskit_version::semver::Version;

    use super::{apply, version_in};

    const MANIFEST: &str = r#"{
    "name": "@acme/app",
    "version": "1.0.0",
    "description": "kept   exactly",
    "dependencies": {
        "@acme/core": "^1.2.0",
        "@acme/util": "workspace:^",
        "@acme/ui": "~1.2.0",
        "left-pad": "^1.0.0"
    },
    "devDependencies": {"@acme/core": "1.2.0", "@acme/tag": "latest"},
    "peerDependencies": { "@acme/core": ">=1.0.0 <3" },
    "optionalDependencies": { "@acme/ui": "npm:other@1" }
}
"#;

    fn version(raw: &str) -> Version {
        Version::parse(raw).unwrap()
    }

    fn floors(entries: &[(&str, &str)]) -> BTreeMap<String, Version> {
        entries
            .iter()
            .map(|(name, raw)| ((*name).to_string(), version(raw)))
            .collect()
    }

    #[test]
    fn reads_the_declared_version() {
        assert_eq!(version_in(MANIFEST, "p").unwrap(), Some(version("1.0.0")));
        assert_eq!(version_in("\u{feff}{\"name\":\"a\"}", "p").unwrap(), None);
    }

    #[test]
    fn malformed_versions_and_documents_are_typed_errors() {
        for text in [r#"{"version": "1.0"}"#, r#"{"version": 1}"#, "[]", "{"] {
            let error = version_in(text, "pkg/package.json").unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidFormat, "{text}");
            assert!(error.to_string().contains("pkg/package.json"), "{error}");
        }
    }

    #[test]
    fn rewrites_the_version_and_simple_ranges_and_nothing_else() {
        let rewritten = apply(
            MANIFEST,
            "p",
            Some(&version("1.1.0")),
            &floors(&[
                ("@acme/core", "1.3.0"),
                ("@acme/util", "2.0.0"),
                ("@acme/ui", "1.2.5"),
                ("@acme/tag", "9.0.0"),
            ]),
        )
        .unwrap()
        .unwrap();
        let expected = MANIFEST
            .replace("\"version\": \"1.0.0\"", "\"version\": \"1.1.0\"")
            .replace("\"@acme/core\": \"^1.2.0\"", "\"@acme/core\": \"^1.3.0\"")
            .replace("\"@acme/ui\": \"~1.2.0\"", "\"@acme/ui\": \"~1.2.5\"")
            .replace("\"@acme/core\": \"1.2.0\"", "\"@acme/core\": \"1.3.0\"");
        assert_eq!(rewritten, expected);
    }

    #[test]
    fn an_unchanged_manifest_is_not_rewritten() {
        assert_eq!(
            apply(MANIFEST, "p", Some(&version("1.0.0")), &BTreeMap::new()).unwrap(),
            None
        );
    }

    #[test]
    fn a_range_the_new_version_leaves_behind_is_a_conflict() {
        let error = apply(MANIFEST, "p", None, &floors(&[("@acme/core", "3.0.0")])).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(
            error
                .to_string()
                .contains("peerDependencies '@acme/core' is '>=1.0.0 <3'"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_version_is_added_after_the_name() {
        let text = "\u{feff}{\n\t\"name\": \"a\",\n\t\"private\": true\n}";
        assert_eq!(
            apply(text, "p", Some(&version("0.1.0")), &BTreeMap::new())
                .unwrap()
                .unwrap(),
            "\u{feff}{\n\t\"name\": \"a\",\n\t\"version\": \"0.1.0\",\n\t\"private\": true\n}"
        );
        let error = apply("{}", "p", Some(&version("0.1.0")), &BTreeMap::new()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput);
    }
}
