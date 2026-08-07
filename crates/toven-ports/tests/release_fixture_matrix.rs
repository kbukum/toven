//! Declarative release-flow fixture coverage.
//!
//! These cases are intentionally kept as TOML files beside the port fixtures:
//! they are consumed by engine and CLI tests without embedding release policy
//! in Rust source.

const CASES: &[(&str, &str)] = &[
    ("toven-self.toml", "shared-tag binary"),
    (
        "rskit-maintainer.toml",
        "independent maintainer-owned libraries",
    ),
    ("mixed-lib-binary.toml", "mixed libraries and binary"),
    ("image-service.toml", "image service"),
    ("go-native.toml", "Go native backing"),
    (
        "go-goreleaser-delegated.toml",
        "GoReleaser delegated backing",
    ),
];

fn fixture(name: &str) -> &'static str {
    match name {
        "toven-self.toml" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/config/release/toven-self.toml"
        )),
        "rskit-maintainer.toml" => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/config/release/rskit-maintainer.toml"
            ))
        }
        "mixed-lib-binary.toml" => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/config/release/mixed-lib-binary.toml"
            ))
        }
        "image-service.toml" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/config/release/image-service.toml"
        )),
        "go-native.toml" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/config/release/go-native.toml"
        )),
        "go-goreleaser-delegated.toml" => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/config/release/go-goreleaser-delegated.toml"
            ))
        }
        _ => panic!("unknown release fixture: {name}"),
    }
}

#[test]
fn every_flow_fixture_is_declarative_and_covers_the_complete_phase_surface() {
    assert_eq!(toven_model::ReleasePhase::ALL.len(), 9);
    for (file, shape) in CASES {
        let raw = fixture(file);
        let value: toml::Value = toml::from_str(raw).expect("fixture is valid TOML");
        assert!(value.get("project").is_some(), "{shape} needs a project");
        assert!(
            value.get("ecosystems").is_some(),
            "{shape} needs ecosystem policy"
        );
        assert!(!raw.trim().is_empty(), "{shape} must not be empty");

        let ecosystems = value["ecosystems"].as_table().expect("ecosystems table");
        for (ecosystem, config) in ecosystems {
            if let Some(release) = config.get("release") {
                let parsed: toven_ports::ReleaseConfig = release
                    .clone()
                    .try_into()
                    .expect("ecosystem release config parses");
                parsed
                    .validate(&format!("ecosystems.{ecosystem}.release"))
                    .expect("ecosystem release config validates");
            }
        }
        if let Some(modules) = value.get("modules").and_then(toml::Value::as_table) {
            for (module, config) in modules {
                if let Some(release) = config.get("release") {
                    let parsed: toven_ports::ReleaseConfig = release
                        .clone()
                        .try_into()
                        .expect("module release config parses");
                    parsed
                        .validate(&format!("modules.{module}.release"))
                        .expect("module release config validates");
                }
            }
        }
    }
}

#[test]
fn shape_specific_policy_is_present_in_the_fixture_matrix() {
    let load = |file: &str| -> toml::Value { toml::from_str(fixture(file)).expect("valid TOML") };

    let toven = load("toven-self.toml");
    assert_eq!(
        toven["ecosystems"]["rust"]["release"]["tag_format"],
        "v{version}".into()
    );
    assert_eq!(
        toven["ecosystems"]["rust"]["release"]["host"]["forge"],
        "github".into()
    );

    let rskit = load("rskit-maintainer.toml");
    assert_eq!(
        rskit["ecosystems"]["rust"]["release"]["entrypoint"],
        "maintainer".into()
    );
    assert_eq!(
        rskit["modules"]["rust:examples"]["release"]["exclude"],
        true.into()
    );
    assert_eq!(
        rskit["modules"]["rust:rskit-suite"]["release"]["umbrella"],
        true.into()
    );

    let mixed = load("mixed-lib-binary.toml");
    assert_eq!(
        mixed["modules"]["rust:app"]["release"]["publish"],
        false.into()
    );
    assert!(
        mixed["modules"]["rust:app"]["release"]["host"]["assets"]
            .as_array()
            .is_some_and(|assets| assets.len() == 2)
    );

    let image = load("image-service.toml");
    assert_eq!(
        image["modules"]["command:api"]["release"]["image"]["registry"],
        "ghcr.io/acme".into()
    );
    assert_eq!(
        image["modules"]["command:api"]["release"]["image"]["mirrors"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let native_go = load("go-native.toml");
    assert!(
        native_go
            .get("ecosystems")
            .and_then(|value| value.get("go"))
            .and_then(|value| value.get("release"))
            .and_then(|value| value.get("phases"))
            .is_none(),
        "the native sibling must use default native phase backing"
    );

    let go = load("go-goreleaser-delegated.toml");
    assert_eq!(
        go["ecosystems"]["go"]["release"]["phases"]["package"]["backing"],
        "delegated".into()
    );
    assert_eq!(
        go["ecosystems"]["go"]["release"]["phases"]["sign"]["backing"],
        "delegated".into()
    );
}

#[test]
fn fixture_catalog_has_stable_names_for_matrix_reports() {
    let mut names = CASES.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CASES.len());
    assert!(names.iter().all(|name| {
        std::path::Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
    }));
}
