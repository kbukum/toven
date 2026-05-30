#![allow(dead_code, missing_docs, unreachable_pub)]

#[path = "smoke/assertions.rs"]
mod assertions;
#[path = "smoke/binary.rs"]
mod binary;
#[path = "smoke/case.rs"]
mod case;
#[path = "smoke/repo_fixture.rs"]
mod repo_fixture;
#[path = "smoke/snapshot.rs"]
mod snapshot;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use case::SmokeCase;

#[test]
fn managed_smoke_cases_match_snapshots() {
    if std::env::var_os("TOVEN_SMOKE_SKIP_MANAGED").is_some() {
        return;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases_dir = root.join("smoke/cases");
    let expected_dir = root.join("smoke/expected");
    let selected_case = std::env::var("TOVEN_SMOKE_CASE").ok();
    let mut matched_cases = 0;

    for case_path in case::managed_case_paths(&cases_dir) {
        let case = SmokeCase::load(&case_path);
        let case_name = case_path
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("case file has a valid stem");
        if selected_case
            .as_deref()
            .is_some_and(|selected| selected != case_name)
        {
            continue;
        }
        matched_cases += 1;

        let fixture = repo_fixture::prepare(root, &case, case_name);
        let mut actual_snapshot = String::new();

        for invocation in case.invocations() {
            let output = binary::run_toven(&fixture.config, &invocation);

            if invocation.should_assert_cargo_waves() {
                assertions::assert_cargo_waves_match_output(&fixture.repo, &output.stdout);
            }

            if invocation.should_assert_affected_modules() {
                let expected_modules =
                    assertions::expected_affected_modules(&fixture.repo, &invocation);
                assertions::assert_affected_output_matches(
                    &fixture.repo,
                    &output.stdout,
                    &expected_modules,
                );
            }

            let normalized = snapshot::normalize_output(&output, &fixture.repo);
            write!(
                actual_snapshot,
                "## {}\n\n{}\n",
                invocation.label(),
                normalized
            )
            .expect("write smoke snapshot string");
        }

        let expected_path = expected_dir.join(format!("{case_name}.snap"));
        if std::env::var_os("TOVEN_UPDATE_SMOKE_SNAPSHOTS").is_some() {
            fs::write(&expected_path, actual_snapshot).expect("write smoke snapshot");
            continue;
        }

        let expected_snapshot = fs::read_to_string(&expected_path)
            .unwrap_or_else(|err| panic!("read {}: {err}", expected_path.display()));
        assert_eq!(
            expected_snapshot,
            actual_snapshot,
            "snapshot mismatch for {}",
            case_path.display()
        );
    }

    if let Some(selected_case) = selected_case {
        assert_eq!(
            1, matched_cases,
            "selected smoke case {selected_case} not found"
        );
    }
}
