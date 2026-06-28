//! Release tag formatting and parsing.
#![allow(unreachable_pub)]

use rskit_version::semver::Version;
use toven_model::ModuleRef;
use toven_ports::TagRef;

/// Format the `<ecosystem>/<name>` prefix that namespaces a module's tags.
///
/// Identity is `ecosystem:name`, so the tag prefix must carry the ecosystem too;
/// otherwise two same-named modules in different ecosystems (e.g. `rust:core`
/// and `go:core`) would collide on the same tag glob and a module could pick up
/// another ecosystem's tag as its baseline. `/` (a valid git refname separator)
/// keeps the tag well-formed.
fn prefix(module: &ModuleRef) -> String {
    format!("{}/{}", module.ecosystem.as_str(), module.name)
}

/// Format the release tag for `module` at `version`
/// (`<ecosystem>/<name>@<version>`).
#[must_use]
pub fn format(module: &ModuleRef, version: &Version) -> String {
    format!("{}@{version}", prefix(module))
}

/// Parse a release tag for `module`, returning its version when it matches.
fn parse(module: &ModuleRef, name: &str) -> Option<Version> {
    let (tag_prefix, raw_version) = name.rsplit_once('@')?;
    if tag_prefix != prefix(module) {
        return None;
    }
    Version::parse(raw_version).ok()
}

/// Select the newest semver tag for `module`.
pub(super) fn latest(module: &ModuleRef, tags: &[TagRef]) -> Option<(Version, TagRef)> {
    tags.iter()
        .filter_map(|tag| parse(module, &tag.name).map(|version| (version, tag.clone())))
        .max_by(|(left, _), (right, _)| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleRef};
    use toven_ports::{Oid, TagRef};

    use super::{latest, parse};

    fn module() -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap()
    }

    #[test]
    fn tag_grammar_uses_ecosystem_qualified_name_and_version() {
        let module = module();
        let version = Version::new(1, 2, 3);

        assert_eq!(super::format(&module, &version), "rust/core@1.2.3");
        assert_eq!(parse(&module, "rust/core@1.2.3"), Some(version));
        // A bare or wrongly-namespaced tag must not match.
        assert_eq!(parse(&module, "core@1.2.3"), None);
        assert_eq!(parse(&module, "rust:core@1.2.3"), None);
    }

    #[test]
    fn parse_disambiguates_same_name_across_ecosystems() {
        let rust_core = module();
        let go_core = ModuleRef::new(EcosystemId::new("go").unwrap(), "core").unwrap();

        // A go:core tag must never be read as the baseline for rust:core.
        assert_eq!(parse(&rust_core, "go/core@2.0.0"), None);
        assert_eq!(
            parse(&go_core, "go/core@2.0.0"),
            Some(Version::new(2, 0, 0))
        );
    }

    #[test]
    fn latest_ignores_non_matching_tags() {
        let module = module();
        let tags = vec![
            TagRef::new("rust/core@0.1.0", Oid::new("a")),
            TagRef::new("go/core@9.9.9", Oid::new("b")),
            TagRef::new("rust/core@0.2.0", Oid::new("c")),
        ];

        let (version, tag) = latest(&module, &tags).expect("latest tag");

        assert_eq!(version, Version::new(0, 2, 0));
        assert_eq!(tag.name, "rust/core@0.2.0");
    }
}
