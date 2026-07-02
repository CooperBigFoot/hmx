//! Process-boundary conformance tests for generated HMX fixtures.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use jsonschema::Validator;
use serde_json::Value;

const ALL_CHECK_IDS: [&str; 9] = ["M1", "M2", "M3", "P1", "R1", "R2", "D1", "MAP1", "F1"];

#[derive(Debug, Clone, Copy)]
enum FixtureKind {
    Valid,
    ReportInvalid { pinned: &'static str },
    StructuralInvalid,
}

#[derive(Debug, Clone, Copy)]
struct Fixture {
    root: &'static str,
    name: &'static str,
    kind: FixtureKind,
}

const FIXTURES: [Fixture; 12] = [
    Fixture { root: "valid", name: "minimal", kind: FixtureKind::Valid },
    Fixture { root: "valid", name: "real-shape-basin", kind: FixtureKind::Valid },
    Fixture { root: "invalid", name: "unknown-format-version", kind: FixtureKind::StructuralInvalid },
    Fixture { root: "invalid", name: "extra-manifest-field", kind: FixtureKind::StructuralInvalid },
    Fixture { root: "invalid", name: "missing-crs", kind: FixtureKind::StructuralInvalid },
    Fixture { root: "invalid", name: "cell-to-gauge-missing-variable", kind: FixtureKind::StructuralInvalid },
    Fixture { root: "invalid", name: "dotdot-substring-path", kind: FixtureKind::ReportInvalid { pinned: "P1" } },
    Fixture { root: "invalid", name: "malformed-registry", kind: FixtureKind::ReportInvalid { pinned: "R1" } },
    Fixture { root: "invalid", name: "undeclared-attribute-field", kind: FixtureKind::ReportInvalid { pinned: "R2" } },
    Fixture { root: "invalid", name: "dangling-mapping-id", kind: FixtureKind::ReportInvalid { pinned: "D1" } },
    Fixture { root: "invalid", name: "mapping-role-non-mapping-format", kind: FixtureKind::ReportInvalid { pinned: "MAP1" } },
    Fixture { root: "invalid", name: "missing-required-column", kind: FixtureKind::ReportInvalid { pinned: "F1" } },
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn schema(file: &str) -> PathBuf {
    repo_root().join("schemas").join(file)
}

fn load_schema(file: &str) -> Validator {
    let path = schema(file);
    let raw =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let document: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{file} must be valid JSON: {e}"));
    jsonschema::validator_for(&document)
        .unwrap_or_else(|e| panic!("{file} must compile as a JSON Schema: {e}"))
}

fn stdout_as_json(stdout: &[u8], what: &str) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|e| panic!("{what} stdout is not valid JSON: {e}"))
}

fn run_hmx(args: &[&str]) -> (i32, Vec<u8>) {
    let output = Command::new(env!("CARGO_BIN_EXE_hmx"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .expect("failed to launch hmx binary");
    let code = output
        .status
        .code()
        .expect("hmx process was terminated by a signal");
    (code, output.stdout)
}

fn package_arg(fixture: Fixture) -> String {
    format!("conformance/{}/{}", fixture.root, fixture.name)
}

fn golden_path(fixture: Fixture, verb: &str) -> PathBuf {
    repo_root()
        .join("conformance/goldens")
        .join(format!("{}.{}.json", fixture.name, verb))
}

fn bless_enabled() -> bool {
    std::env::var_os("HMX_BLESS").is_some()
}

fn assert_or_bless(path: &Path, stdout: &[u8]) {
    if bless_enabled() {
        fs::write(path, stdout).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    } else {
        let expected =
            fs::read(path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()));
        assert_eq!(
            expected,
            stdout,
            "stdout differs from golden {}",
            path.display()
        );
    }
}

fn generated_fixtures_exist() -> bool {
    repo_root().join("conformance/valid/minimal/manifest.json").exists()
}

fn directory_names(root: &Path) -> BTreeSet<String> {
    fs::read_dir(root)
        .unwrap_or_else(|e| panic!("read {}: {e}", root.display()))
        .filter_map(|entry| {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if path.is_dir() {
                Some(entry.file_name().to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect()
}

fn assert_declared_fixture_dirs() {
    for root in ["valid", "invalid"] {
        let actual = directory_names(&repo_root().join("conformance").join(root));
        let expected = FIXTURES
            .iter()
            .filter(|fixture| fixture.root == root)
            .map(|fixture| fixture.name.to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "conformance/{root} fixture drift");
    }
}

fn validate_schema(validator: &Validator, stdout: &[u8], what: &str) -> Value {
    let value = stdout_as_json(stdout, what);
    if let Err(error) = validator.validate(&value) {
        panic!("{what} stdout must validate against schema: {error}");
    }
    value
}

fn assert_all_checks_pass(value: &Value, fixture: &str) {
    assert_eq!(value.get("conformant").and_then(Value::as_bool), Some(true));
    let checks = value
        .get("checks")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{fixture}: checks must be an array"));
    assert_eq!(checks.len(), ALL_CHECK_IDS.len(), "{fixture}: check count drift");
    for (check, expected_id) in checks.iter().zip(ALL_CHECK_IDS) {
        assert_eq!(check.get("id").and_then(Value::as_str), Some(expected_id));
        assert_eq!(check.get("status").and_then(Value::as_str), Some("ran"));
        assert_eq!(check.get("result").and_then(Value::as_str), Some("pass"));
    }
}

fn assert_pinned_only_fails(value: &Value, pinned: &str, fixture: &str) {
    assert_eq!(value.get("conformant").and_then(Value::as_bool), Some(false));
    let checks = value
        .get("checks")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{fixture}: checks must be an array"));
    let mut failures = Vec::new();
    for check in checks {
        if check.get("result").and_then(Value::as_str) == Some("fail") {
            failures.push(check.get("id").and_then(Value::as_str).unwrap_or("<missing>"));
        }
    }
    assert_eq!(failures, vec![pinned], "{fixture}: collateral check failure");
}

#[test]
fn generated_conformance_suite_matches_goldens() {
    if !generated_fixtures_exist() {
        eprintln!("skipping conformance suite: generated fixtures are absent");
        return;
    }

    assert_declared_fixture_dirs();
    let describe_validator = load_schema("describe.schema.json");
    let validate_validator = load_schema("validate.schema.json");

    for fixture in FIXTURES {
        let package = package_arg(fixture);
        match fixture.kind {
            FixtureKind::Valid => {
                let (describe_code, describe_stdout) = run_hmx(&["describe", &package]);
                assert_eq!(describe_code, 0, "{package} describe must exit 0");
                validate_schema(&describe_validator, &describe_stdout, &format!("{package} describe"));
                assert_or_bless(&golden_path(fixture, "describe"), &describe_stdout);

                let (validate_code, validate_stdout) = run_hmx(&["validate", &package]);
                assert_eq!(validate_code, 0, "{package} validate must exit 0");
                let value = validate_schema(&validate_validator, &validate_stdout, &format!("{package} validate"));
                assert_all_checks_pass(&value, &package);
                assert_or_bless(&golden_path(fixture, "validate"), &validate_stdout);
            }
            FixtureKind::ReportInvalid { pinned } => {
                let (code, stdout) = run_hmx(&["validate", &package]);
                assert_eq!(code, 1, "{package} validate must exit 1");
                let value = validate_schema(&validate_validator, &stdout, &format!("{package} validate"));
                assert_pinned_only_fails(&value, pinned, &package);
                assert_or_bless(&golden_path(fixture, "validate"), &stdout);
            }
            FixtureKind::StructuralInvalid => {
                for verb in ["describe", "validate"] {
                    let (code, stdout) = run_hmx(&[verb, &package]);
                    assert_eq!(code, 2, "{package} {verb} must exit 2");
                    assert!(
                        stdout.is_empty(),
                        "{package} {verb} structural error emitted stdout: {}",
                        String::from_utf8_lossy(&stdout)
                    );
                }
            }
        }
    }
}
