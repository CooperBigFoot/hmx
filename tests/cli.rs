//! Process-boundary tests for the `hmx` CLI.

use std::path::PathBuf;
use std::process::Command;

use jsonschema::Validator;
use serde_json::Value;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn fixture_arg(rel: &str) -> String {
    fixture(rel)
        .to_str()
        .expect("fixture path is valid UTF-8")
        .to_string()
}

fn run_hmx_full(args: &[&str]) -> (i32, Vec<u8>) {
    let output = Command::new(env!("CARGO_BIN_EXE_hmx"))
        .args(args)
        .output()
        .expect("failed to launch hmx binary");
    let code = output
        .status
        .code()
        .expect("hmx process was terminated by a signal");
    (code, output.stdout)
}

fn run_hmx(args: &[&str]) -> Vec<u8> {
    run_hmx_full(args).1
}

fn schema(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("schemas")
        .join(file)
}

fn load_schema(file: &str) -> Validator {
    let path = schema(file);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let document: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{file} must be valid JSON: {e}"));
    jsonschema::validator_for(&document)
        .unwrap_or_else(|e| panic!("{file} must compile as a JSON Schema: {e}"))
}

fn stdout_as_json(stdout: &[u8], what: &str) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|e| panic!("{what} stdout is not valid JSON: {e}"))
}

fn assert_empty_stdout(stdout: &[u8]) {
    assert!(
        stdout.is_empty(),
        "exit-2 errors emit no JSON on stdout, got: {}",
        String::from_utf8_lossy(stdout)
    );
}

#[test]
fn validate_valid_exits_zero_conformant_true() {
    let (code, stdout) = run_hmx_full(&["validate", &fixture_arg("tests/fixtures/valid")]);

    assert_eq!(code, 0, "valid package must exit 0");
    let value = stdout_as_json(&stdout, "validate valid");
    assert_eq!(value.get("conformant").and_then(Value::as_bool), Some(true));
}

#[test]
fn validate_nonconformant_exits_one_conformant_false() {
    let (code, stdout) = run_hmx_full(&["validate", &fixture_arg("tests/fixtures/nonconformant")]);

    assert_eq!(code, 1, "non-conformant report must exit 1");
    let value = stdout_as_json(&stdout, "validate nonconformant");
    assert_eq!(value.get("conformant").and_then(Value::as_bool), Some(false));
}

#[test]
fn validate_malformed_manifest_exits_two_empty_stdout() {
    let (code, stdout) =
        run_hmx_full(&["validate", &fixture_arg("tests/fixtures/malformed-manifest")]);

    assert_eq!(code, 2, "malformed manifest must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn validate_unknown_format_version_exits_two_empty_stdout() {
    let (code, stdout) =
        run_hmx_full(&["validate", &fixture_arg("tests/fixtures/unknown-format-version")]);

    assert_eq!(code, 2, "unknown format_version must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn validate_nonexistent_path_exits_two_empty_stdout() {
    let (code, stdout) =
        run_hmx_full(&["validate", &fixture_arg("tests/fixtures/does-not-exist")]);

    assert_eq!(code, 2, "nonexistent package path must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn validate_without_path_exits_two() {
    let (code, _stdout) = run_hmx_full(&["validate"]);

    assert_eq!(code, 2, "missing path is a clap usage error");
}

#[test]
fn no_subcommand_exits_two() {
    let (code, _stdout) = run_hmx_full(&[]);

    assert_eq!(code, 2, "missing subcommand is a clap usage error");
}

#[test]
fn describe_valid_exits_zero_with_content_hash() {
    let (code, stdout) = run_hmx_full(&["describe", &fixture_arg("tests/fixtures/valid")]);

    assert_eq!(code, 0, "valid package describe must exit 0");
    let value = stdout_as_json(&stdout, "describe valid");
    let hash = value
        .get("content_hash")
        .and_then(Value::as_object)
        .expect("content_hash is an object");
    assert_eq!(hash.get("algo").and_then(Value::as_str), Some("sha256"));
    let hash_value = hash
        .get("value")
        .and_then(Value::as_str)
        .expect("content_hash.value is a string");
    assert_eq!(hash_value.len(), 64, "content_hash.value is 64 hex characters");
    assert!(
        hash_value.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "content_hash.value is lowercase hex"
    );
}

#[test]
fn describe_unknown_format_version_exits_two_empty_stdout() {
    let (code, stdout) =
        run_hmx_full(&["describe", &fixture_arg("tests/fixtures/unknown-format-version")]);

    assert_eq!(code, 2, "unknown format_version must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn describe_nonexistent_path_exits_two_empty_stdout() {
    let (code, stdout) =
        run_hmx_full(&["describe", &fixture_arg("tests/fixtures/does-not-exist")]);

    assert_eq!(code, 2, "nonexistent package path must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn validate_stdout_validates_against_schema() {
    let validator = load_schema("validate.schema.json");

    for fixture in ["tests/fixtures/valid", "tests/fixtures/nonconformant"] {
        let stdout = run_hmx(&["validate", &fixture_arg(fixture)]);
        let value = stdout_as_json(&stdout, fixture);
        if let Err(error) = validator.validate(&value) {
            panic!("{fixture} validate stdout must validate against validate.schema.json: {error}");
        }
    }
}

#[test]
fn describe_stdout_validates_against_schema() {
    let validator = load_schema("describe.schema.json");
    let stdout = run_hmx(&["describe", &fixture_arg("tests/fixtures/valid")]);
    let value = stdout_as_json(&stdout, "describe valid");

    if let Err(error) = validator.validate(&value) {
        panic!("describe stdout must validate against describe.schema.json: {error}");
    }
}
