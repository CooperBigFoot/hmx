//! Process-boundary tests for the `hmx` CLI.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use jsonschema::Validator;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hmx-cli-derive-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

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

fn run_hmx_owned(args: Vec<OsString>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hmx"))
        .args(args)
        .output()
        .expect("failed to launch hmx binary")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination)
        .unwrap_or_else(|error| panic!("create {}: {error}", destination.display()));
    for entry in
        fs::read_dir(source).unwrap_or_else(|error| panic!("read {}: {error}", source.display()))
    {
        let entry = entry.expect("read directory entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().expect("read file type").is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap_or_else(|error| {
                panic!(
                    "copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            });
        }
    }
}

fn read_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("parse {} as JSON: {error}", path.display()))
}

fn artifact_by_role<'a>(manifest: &'a Value, role: &str) -> &'a Value {
    manifest["artifacts"]
        .as_array()
        .expect("manifest artifacts is an array")
        .iter()
        .find(|artifact| artifact["role"] == role)
        .unwrap_or_else(|| panic!("manifest declares artifact role {role}"))
}

fn artifact_by_variable<'a>(manifest: &'a Value, variable: &str) -> &'a Value {
    manifest["artifacts"]
        .as_array()
        .expect("manifest artifacts is an array")
        .iter()
        .find(|artifact| artifact["variable"] == variable)
        .unwrap_or_else(|| panic!("manifest declares artifact variable {variable}"))
}

fn digest_and_size(path: &Path) -> (String, u64) {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    (format!("{:x}", Sha256::digest(&bytes)), bytes.len() as u64)
}

fn assert_valid_package(path: &Path) {
    let output = run_hmx_owned(vec!["validate".into(), path.as_os_str().to_owned()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "validate stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout_as_json(&output.stdout, "validate derived")["conformant"],
        true
    );
}

fn describe(path: &Path) -> Value {
    let output = run_hmx_owned(vec!["describe".into(), path.as_os_str().to_owned()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "describe stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout_as_json(&output.stdout, "describe derived")
}

fn assert_failed_derive(output: &Output, required_stderr: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_empty_stdout(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in required_stderr {
        assert!(
            stderr.contains(required),
            "stderr lacks {required:?}: {stderr}"
        );
    }
}

fn assert_failed_command(output: &Output, required_stderr: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_empty_stdout(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in required_stderr {
        assert!(
            stderr.contains(required),
            "stderr lacks {required:?}: {stderr}"
        );
    }
}

fn derive_fixture(rel: &str) -> PathBuf {
    fixture(&format!("tests/fixtures/derive/{rel}"))
}

fn materialize_fixture() -> PathBuf {
    derive_fixture("base")
}

fn run_materialize(base: &Path, out: &Path, record: &Path) -> Output {
    run_hmx_owned(vec![
        "materialize".into(),
        base.as_os_str().to_owned(),
        out.as_os_str().to_owned(),
        "--record".into(),
        record.as_os_str().to_owned(),
    ])
}

fn decode_tiff(path: &Path) -> (u16, Vec<f64>) {
    let mut decoder = Decoder::new(BufReader::new(
        File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display())),
    ))
    .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()));
    assert_eq!(decoder.dimensions().expect("read TIFF dimensions"), (2, 2));
    let samples = decoder
        .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
        .expect("read SamplesPerPixel")
        .unwrap_or(1);
    let values = match decoder.read_image().expect("read TIFF pixels") {
        DecodingResult::F64(values) => values,
        other => panic!("expected f64 decoding, got {other:?}"),
    };
    (samples, values)
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, current: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(current).expect("read package directory") {
            let entry = entry.expect("read package entry");
            if entry.file_type().expect("package entry type").is_dir() {
                visit(root, &entry.path(), files);
            } else {
                files.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("relative package path")
                        .to_owned(),
                );
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort();
    files
}

fn assert_no_materialize_temporary_siblings(parent: &Path) {
    for entry in fs::read_dir(parent).expect("read temporary parent") {
        let name = entry.expect("read temporary entry").file_name();
        let name = name.to_string_lossy();
        assert!(!name.contains("hmx-staging"), "staging leak: {name}");
        assert!(!name.contains("hmx-record"), "record leak: {name}");
    }
}

fn assert_one_terminal_newline(bytes: &[u8]) {
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_ne!(bytes.get(bytes.len().saturating_sub(2)), Some(&b'\n'));
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
    serde_json::from_slice(stdout)
        .unwrap_or_else(|e| panic!("{what} stdout is not valid JSON: {e}"))
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
    assert_eq!(
        value.get("conformant").and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn validate_malformed_manifest_exits_two_empty_stdout() {
    let (code, stdout) = run_hmx_full(&[
        "validate",
        &fixture_arg("tests/fixtures/malformed-manifest"),
    ]);

    assert_eq!(code, 2, "malformed manifest must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn validate_legacy_0_1_format_version_exits_two_empty_stdout() {
    let (code, stdout) = run_hmx_full(&[
        "validate",
        &fixture_arg("tests/fixtures/unknown-format-version"),
    ]);

    assert_eq!(code, 2, "legacy 0.1 format_version must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn validate_nonexistent_path_exits_two_empty_stdout() {
    let (code, stdout) = run_hmx_full(&["validate", &fixture_arg("tests/fixtures/does-not-exist")]);

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
    assert_eq!(
        hash_value.len(),
        64,
        "content_hash.value is 64 hex characters"
    );
    assert!(
        hash_value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "content_hash.value is lowercase hex"
    );
}

#[test]
fn describe_legacy_0_1_format_version_exits_two_empty_stdout() {
    let (code, stdout) = run_hmx_full(&[
        "describe",
        &fixture_arg("tests/fixtures/unknown-format-version"),
    ]);

    assert_eq!(code, 2, "legacy 0.1 format_version must exit 2");
    assert_empty_stdout(&stdout);
}

#[test]
fn describe_nonexistent_path_exits_two_empty_stdout() {
    let (code, stdout) = run_hmx_full(&["describe", &fixture_arg("tests/fixtures/does-not-exist")]);

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

#[test]
fn derive_sets_scalars_and_layers_as_standalone_packages() {
    let temp = TempDir::new();
    let base = derive_fixture("base");
    let out = temp.path().join("out-scalar");
    let record_path = temp.path().join("scalar-record.json");
    let output = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        out.as_os_str().to_owned(),
        "--name".into(),
        "scalar-derived".into(),
        "--set".into(),
        "cell.snow_melt_threshold_c=0.7".into(),
        "--record".into(),
        record_path.as_os_str().to_owned(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "derive stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ERROR"));
    let record_bytes = fs::read(&record_path).expect("read external record");
    assert_eq!(output.stdout, record_bytes);
    assert_one_terminal_newline(&record_bytes);
    let record = stdout_as_json(&record_bytes, "scalar derive");
    if let Err(error) = load_schema("derive.schema.json").validate(&record) {
        panic!("derive record must validate against derive.schema.json: {error}");
    }
    assert_eq!(record["derived_name"], "scalar-derived");
    assert_eq!(record["tool_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(record["non_parameter_overrides"], json!([]));
    assert_eq!(
        record["replaced"].as_array().expect("replaced array").len(),
        1
    );
    let replacement = &record["replaced"][0];
    assert_eq!(replacement["artifact_role"], "parameter.scalars");
    assert_eq!(
        replacement["field_ids"],
        json!(["cell.snow_melt_threshold_c"])
    );

    let base_manifest = read_json(&base.join("manifest.json"));
    let derived_manifest = read_json(&out.join("manifest.json"));
    let base_scalar = artifact_by_role(&base_manifest, "parameter.scalars");
    let derived_scalar = artifact_by_role(&derived_manifest, "parameter.scalars");
    let scalar_bytes = fs::read(out.join(derived_scalar["path"].as_str().expect("scalar path")))
        .expect("read derived scalars");
    assert_eq!(
        scalar_bytes,
        b"{\"cell.snow_melt_threshold_c\":0.7,\"cell.soil_layer_capacity_mm\":[100.0,150.0,200.0]}\n"
    );
    let (scalar_digest, scalar_size) = digest_and_size(&out.join("parameter/scalars.json"));
    assert_eq!(replacement["old_sha256"], base_scalar["sha256"]);
    assert_eq!(replacement["new_sha256"], scalar_digest);
    assert_eq!(derived_scalar["sha256"], scalar_digest);
    assert_eq!(derived_scalar["size_bytes"], scalar_size);

    let mut changed_roles = Vec::new();
    for base_artifact in base_manifest["artifacts"]
        .as_array()
        .expect("base artifacts")
    {
        let role = base_artifact["role"].as_str().expect("artifact role");
        let derived_artifact = artifact_by_role(&derived_manifest, role);
        if base_artifact["sha256"] != derived_artifact["sha256"] {
            changed_roles.push(role);
        } else {
            let base_bytes =
                fs::read(base.join(base_artifact["path"].as_str().expect("base path")))
                    .expect("read base artifact");
            let derived_bytes =
                fs::read(out.join(derived_artifact["path"].as_str().expect("derived path")))
                    .expect("read derived artifact");
            assert_eq!(derived_bytes, base_bytes, "unchanged artifact {role}");
        }
    }
    assert_eq!(changed_roles, ["parameter.scalars"]);

    let mut normalized = derived_manifest.clone();
    normalized["name"] = base_manifest["name"].clone();
    normalized["created_at"] = base_manifest["created_at"].clone();
    for artifact in normalized["artifacts"].as_array_mut().expect("artifacts") {
        let base_artifact = artifact_by_role(
            &base_manifest,
            artifact["role"].as_str().expect("artifact role"),
        );
        artifact["sha256"] = base_artifact["sha256"].clone();
        artifact["size_bytes"] = base_artifact["size_bytes"].clone();
    }
    assert_eq!(
        normalized, base_manifest,
        "only permitted manifest facts differ"
    );
    assert_eq!(derived_manifest["name"], "scalar-derived");
    let created_at = derived_manifest["created_at"]
        .as_str()
        .expect("created_at string");
    assert_ne!(created_at, "2026-07-10T00:00:00Z");
    assert_eq!(
        created_at,
        record["created_at"].as_str().expect("record created_at")
    );
    assert!(created_at.ends_with('Z'));

    let mut package_files = Vec::new();
    fn collect_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(current).expect("read package directory") {
            let entry = entry.expect("read package entry");
            if entry.file_type().expect("package entry type").is_dir() {
                collect_files(root, &entry.path(), files);
            } else {
                files.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("relative path")
                        .to_owned(),
                );
            }
        }
    }
    collect_files(&out, &out, &mut package_files);
    package_files.sort();
    let mut declared_files = derived_manifest["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .map(|artifact| PathBuf::from(artifact["path"].as_str().expect("artifact path")))
        .collect::<Vec<_>>();
    declared_files.push(PathBuf::from("manifest.json"));
    declared_files.sort();
    assert_eq!(package_files, declared_files);
    assert_valid_package(&out);
    assert_eq!(
        describe(&out)["content_hash"],
        record["derived_content_hash"]
    );

    let layers_out = temp.path().join("out-layers");
    let layers_output = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        layers_out.as_os_str().to_owned(),
        "--name".into(),
        "layers-derived".into(),
        "--set".into(),
        "cell.soil_layer_capacity_mm=[110.0,160.0,210.0]".into(),
    ]);
    assert_eq!(
        layers_output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&layers_output.stderr)
    );
    let layers_record = stdout_as_json(&layers_output.stdout, "layer derive");
    assert_eq!(
        layers_record["replaced"][0]["field_ids"],
        json!(["cell.soil_layer_capacity_mm"])
    );
    let layer_bytes =
        fs::read(layers_out.join("parameter/scalars.json")).expect("read layer scalars");
    assert_one_terminal_newline(&layer_bytes);
    assert_eq!(
        stdout_as_json(&layer_bytes, "layer scalars")["cell.soil_layer_capacity_mm"],
        json!([110.0, 160.0, 210.0])
    );
    assert_valid_package(&layers_out);
}

#[test]
fn derive_replaces_parameter_raster_using_declared_old_digest() {
    let temp = TempDir::new();
    let base = temp.path().join("base");
    copy_tree(&derive_fixture("base"), &base);
    let manifest_path = base.join("manifest.json");
    let mut manifest = read_json(&manifest_path);
    let sentinel = "d".repeat(64);
    artifact_by_role(&manifest, "parameter.spatial_coefficient");
    manifest["artifacts"]
        .as_array_mut()
        .expect("artifacts")
        .iter_mut()
        .find(|artifact| artifact["role"] == "parameter.spatial_coefficient")
        .expect("spatial artifact")["sha256"] = Value::String(sentinel.clone());
    fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("serialize manifest"),
    )
    .expect("write copied manifest");
    let base_cog = base.join("parameter/spatial_coefficient.tif");
    assert_ne!(digest_and_size(&base_cog).0, sentinel);
    assert_valid_package(&base);

    let input = derive_fixture("inputs/spatial_replacement.tif");
    let out = temp.path().join("out-raster");
    let output = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        out.as_os_str().to_owned(),
        "--name".into(),
        "raster-derived".into(),
        "--replace".into(),
        format!("cell.spatial_coefficient={}", input.display()).into(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_valid_package(&out);
    let derived_cog = out.join("parameter/spatial_coefficient.tif");
    assert_eq!(
        fs::read(&derived_cog).expect("derived COG"),
        fs::read(&input).expect("input COG")
    );
    let (digest, size) = digest_and_size(&input);
    let derived_manifest = read_json(&out.join("manifest.json"));
    let artifact = artifact_by_role(&derived_manifest, "parameter.spatial_coefficient");
    assert_eq!(artifact["sha256"], digest);
    assert_eq!(artifact["size_bytes"], size);
    let record = stdout_as_json(&output.stdout, "raster derive");
    assert_eq!(record["replaced"].as_array().expect("replaced").len(), 1);
    assert_eq!(
        record["replaced"][0]["artifact_role"],
        "parameter.spatial_coefficient"
    );
    assert_eq!(
        record["replaced"][0]["field_ids"],
        json!(["cell.spatial_coefficient"])
    );
    assert_eq!(record["replaced"][0]["new_sha256"], digest);
    assert_eq!(record["replaced"][0]["old_sha256"], sentinel);
}

#[test]
fn derive_refuses_undeclared_ids_and_multiband_cogs() {
    let temp = TempDir::new();
    let base = derive_fixture("base");
    let input = derive_fixture("inputs/spatial_replacement.tif");
    let set_out = temp.path().join("set-undeclared");
    let set = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        set_out.as_os_str().to_owned(),
        "--name".into(),
        "bad-set".into(),
        "--set".into(),
        "cell.not_declared=1.0".into(),
    ]);
    assert_failed_derive(&set, &["resolving --set exact FieldId `cell.not_declared`"]);
    assert!(!set_out.exists());

    let replace_out = temp.path().join("replace-undeclared");
    let replace = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        replace_out.as_os_str().to_owned(),
        "--name".into(),
        "bad-replace".into(),
        "--replace".into(),
        format!("cell.not_declared={}", input.display()).into(),
    ]);
    assert_failed_derive(
        &replace,
        &["resolving undeclared exact FieldId `cell.not_declared`"],
    );
    assert!(!replace_out.exists());

    let multiband = derive_fixture("inputs/multiband_replacement.tif");
    let multiband_out = temp.path().join("multiband");
    let multiband_result = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        multiband_out.as_os_str().to_owned(),
        "--name".into(),
        "bad-bands".into(),
        "--replace".into(),
        format!("cell.spatial_coefficient={}", multiband.display()).into(),
        "--allow-non-parameter".into(),
    ]);
    let input_path = multiband.to_string_lossy();
    assert_failed_derive(
        &multiband_result,
        &[
            "replacement COG",
            "has 2 bands; exactly one required",
            &input_path,
        ],
    );
    assert!(!multiband_out.exists());
}

#[test]
fn derive_requires_and_records_non_parameter_override() {
    let temp = TempDir::new();
    let base = derive_fixture("base");
    let input = derive_fixture("inputs/forcing_replacement.tif");
    let denied_out = temp.path().join("out-forcing-denied");
    let denied = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        denied_out.as_os_str().to_owned(),
        "--name".into(),
        "forcing-denied".into(),
        "--replace".into(),
        format!("cell.air_temperature_c={}", input.display()).into(),
    ]);
    assert_failed_derive(
        &denied,
        &["replacement affected non-parameter fields: `cell.air_temperature_c` (forcing)"],
    );
    assert!(!denied_out.exists());

    let allowed_out = temp.path().join("out-forcing-allowed");
    let allowed = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        allowed_out.as_os_str().to_owned(),
        "--name".into(),
        "forcing-allowed".into(),
        "--replace".into(),
        format!("cell.air_temperature_c={}", input.display()).into(),
        "--allow-non-parameter".into(),
    ]);
    assert_eq!(
        allowed.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let stderr = String::from_utf8_lossy(&allowed.stderr);
    for expected in [
        "WARN",
        "allowing non-parameter artifact replacement",
        "forcing.air_temperature",
        "cell.air_temperature_c",
    ] {
        assert!(
            stderr.contains(expected),
            "stderr lacks {expected:?}: {stderr}"
        );
    }
    let record = stdout_as_json(&allowed.stdout, "forcing derive");
    assert_eq!(
        record["non_parameter_overrides"],
        json!(["cell.air_temperature_c"])
    );
    assert_eq!(record["replaced"].as_array().expect("replaced").len(), 1);
    assert_eq!(
        record["replaced"][0]["field_ids"],
        json!(["cell.air_temperature_c"])
    );
    let derived = allowed_out.join("forcing/air_temperature.tif");
    assert_eq!(
        fs::read(&derived).expect("derived forcing"),
        fs::read(&input).expect("input forcing")
    );
    let (digest, size) = digest_and_size(&input);
    let manifest = read_json(&allowed_out.join("manifest.json"));
    let artifact = artifact_by_role(&manifest, "forcing.air_temperature");
    assert_eq!(artifact["sha256"], digest);
    assert_eq!(artifact["size_bytes"], size);
    assert_valid_package(&allowed_out);
}

#[test]
fn derive_pins_domain_attribute_replacement_semantics() {
    let temp = TempDir::new();
    let base = derive_fixture("base");
    let drops_target = derive_fixture("inputs/table_drops_target.parquet");
    let denied_out = temp.path().join("table-drops-target");
    let denied = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        denied_out.as_os_str().to_owned(),
        "--name".into(),
        "drops-target".into(),
        "--replace".into(),
        format!("cell.table_parameter={}", drops_target.display()).into(),
    ]);
    assert_failed_derive(
        &denied,
        &["replacement domain attributes dropped targeted FieldId `cell.table_parameter`"],
    );
    assert!(!denied_out.exists());

    let union = derive_fixture("inputs/table_union.parquet");
    let union_out = temp.path().join("table-union");
    let allowed = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        union_out.as_os_str().to_owned(),
        "--name".into(),
        "table-union".into(),
        "--replace".into(),
        format!("cell.table_parameter={}", union.display()).into(),
        "--allow-non-parameter".into(),
    ]);
    assert_eq!(
        allowed.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let record = stdout_as_json(&allowed.stdout, "table union derive");
    assert_eq!(record["replaced"].as_array().expect("replaced").len(), 1);
    assert_eq!(
        record["replaced"][0]["artifact_role"],
        "attributes.calibration"
    );
    assert_eq!(
        record["replaced"][0]["field_ids"],
        json!([
            "cell.table_new_forcing",
            "cell.table_old_forcing",
            "cell.table_parameter"
        ])
    );
    assert_eq!(
        record["non_parameter_overrides"],
        json!(["cell.table_new_forcing", "cell.table_old_forcing"])
    );
    let stderr = String::from_utf8_lossy(&allowed.stderr);
    assert!(
        stderr.contains("cell.table_new_forcing"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("cell.table_old_forcing"),
        "stderr: {stderr}"
    );
    assert_valid_package(&union_out);
}

#[test]
fn derive_enforces_record_safety_and_cleans_staged_failure() {
    let temp = TempDir::new();
    let base = derive_fixture("base");
    let equal_out = temp.path().join("record-equals-out");
    let equal = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        equal_out.as_os_str().to_owned(),
        "--name".into(),
        "record-equal".into(),
        "--set".into(),
        "cell.snow_melt_threshold_c=0.7".into(),
        "--record".into(),
        equal_out.as_os_str().to_owned(),
    ]);
    assert_failed_derive(&equal, &["record path must be outside output package"]);
    assert!(!equal_out.exists());

    let child_out = temp.path().join("record-child-out");
    let child_record = child_out.join("record.json");
    let child = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        child_out.as_os_str().to_owned(),
        "--name".into(),
        "record-child".into(),
        "--set".into(),
        "cell.snow_melt_threshold_c=0.7".into(),
        "--record".into(),
        child_record.as_os_str().to_owned(),
    ]);
    let out_text = child_out.to_string_lossy();
    assert_failed_derive(&child, &["resolving existing record parent", &out_text]);
    let child_stderr = String::from_utf8_lossy(&child.stderr);
    assert!(
        !child_stderr.contains(child_record.to_string_lossy().as_ref()),
        "stderr names descendant: {child_stderr}"
    );
    assert!(!child_out.exists());
    assert!(!child_record.exists());

    let unknown = derive_fixture("inputs/table_unknown.parquet");
    let failed_out = temp.path().join("staged-invalid");
    let failed_record = temp.path().join("staged-invalid-record.json");
    let failed = run_hmx_owned(vec![
        "derive".into(),
        base.as_os_str().to_owned(),
        failed_out.as_os_str().to_owned(),
        "--name".into(),
        "staged-invalid".into(),
        "--replace".into(),
        format!("cell.table_parameter={}", unknown.display()).into(),
        "--allow-non-parameter".into(),
        "--record".into(),
        failed_record.as_os_str().to_owned(),
    ]);
    assert_failed_derive(
        &failed,
        &[
            "staged derived package is non-conformant",
            "R2",
            "cell.unknown_column",
        ],
    );
    assert!(!failed_out.exists());
    assert!(!failed_record.exists());
    for entry in fs::read_dir(temp.path()).expect("read temporary parent") {
        let name = entry.expect("read temporary entry").file_name();
        let name = name.to_string_lossy();
        assert!(!name.contains("hmx-staging"), "staging leak: {name}");
        assert!(!name.contains("hmx-record"), "record leak: {name}");
    }
}

#[test]
fn materialize_proves_rasters_only_package_and_record_contract() {
    const SCALAR_ID: &str = "cell.snow_melt_threshold_c";
    const SCALAR_ROLE: &str =
        "parameter.materialized.8f9064ae12b0f4fbd785e178faeccef0ab390973a5eb5dd56fa3f380dd7fee4c";
    const SCALAR_PATH: &str = "parameter/materialized/8f9064ae12b0f4fbd785e178faeccef0ab390973a5eb5dd56fa3f380dd7fee4c.tif";
    const LAYER_ID: &str = "cell.soil_layer_capacity_mm";
    const LAYER_ROLE: &str =
        "parameter.materialized.569b6a9187844ced042e4395ebde5b900a6792f122cc0ecd934ffa4831dde81e";
    const LAYER_PATH: &str = "parameter/materialized/569b6a9187844ced042e4395ebde5b900a6792f122cc0ecd934ffa4831dde81e.tif";
    const SCALARS_DIGEST: &str = "47c039db0ac70d52f67385061afca37bcb4d41ba718090819adefb9e70b9fa97";

    let temp = TempDir::new();
    let base = materialize_fixture();
    let out = temp.path().join("materialized");
    let record_path = temp.path().join("materialize-record.json");
    let output = run_materialize(&base, &out, &record_path);
    assert_eq!(
        output.status.code(),
        Some(0),
        "materialize stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("ERROR"),
        "materialize stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let record_bytes = fs::read(&record_path).expect("read materialize record");
    assert_eq!(output.stdout, record_bytes);
    assert_one_terminal_newline(&record_bytes);
    let record = stdout_as_json(&record_bytes, "materialize record");
    if let Err(error) = load_schema("derive.schema.json").validate(&record) {
        panic!("materialize record must validate against derive.schema.json: {error}");
    }

    let base_manifest = read_json(&base.join("manifest.json"));
    let out_manifest = read_json(&out.join("manifest.json"));
    assert_eq!(out_manifest["name"], "derive-base");
    assert_ne!(out_manifest["created_at"], base_manifest["created_at"]);
    assert_eq!(out_manifest["created_at"], record["created_at"]);
    assert!(!out.join("parameter/scalars.json").exists());
    assert!(
        out_manifest["artifacts"]
            .as_array()
            .expect("output artifacts")
            .iter()
            .all(|artifact| artifact["role"] != "parameter.scalars"
                && artifact["format"] != "hmx/parameter_scalars_v1")
    );

    let generated = out_manifest["artifacts"]
        .as_array()
        .expect("output artifacts")
        .iter()
        .filter(|artifact| {
            artifact["role"]
                .as_str()
                .is_some_and(|role| role.starts_with("parameter.materialized."))
        })
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), 2);
    for (field_id, role, path) in [
        (SCALAR_ID, SCALAR_ROLE, SCALAR_PATH),
        (LAYER_ID, LAYER_ROLE, LAYER_PATH),
    ] {
        let artifact = artifact_by_variable(&out_manifest, field_id);
        assert_eq!(artifact["role"], role);
        assert_eq!(artifact["path"], path);
        assert_eq!(artifact["format"], "cog");
        assert_eq!(artifact["variable"], field_id);
        assert_eq!(artifact["crs"], "EPSG:32645");
        let (digest, size) = digest_and_size(&out.join(path));
        assert_eq!(artifact["sha256"], digest);
        assert_eq!(artifact["size_bytes"], size);
    }

    for base_artifact in base_manifest["artifacts"]
        .as_array()
        .expect("base artifacts")
        .iter()
        .filter(|artifact| artifact["role"] != "parameter.scalars")
    {
        let role = base_artifact["role"].as_str().expect("base artifact role");
        let retained = artifact_by_role(&out_manifest, role);
        assert_eq!(retained, base_artifact, "declaration changed for {role}");
        let path = base_artifact["path"].as_str().expect("base artifact path");
        assert_eq!(
            fs::read(base.join(path)).expect("read base artifact"),
            fs::read(out.join(path)).expect("read output artifact"),
            "bytes changed for {role}"
        );
    }

    let mut declared_files = out_manifest["artifacts"]
        .as_array()
        .expect("output artifacts")
        .iter()
        .map(|artifact| PathBuf::from(artifact["path"].as_str().expect("artifact path")))
        .collect::<Vec<_>>();
    declared_files.push(PathBuf::from("manifest.json"));
    declared_files.sort();
    assert_eq!(collect_files(&out), declared_files);

    assert_eq!(decode_tiff(&out.join(SCALAR_PATH)), (1, vec![0.5; 4]));
    assert_eq!(
        decode_tiff(&out.join(LAYER_PATH)),
        (
            3,
            vec![
                100.0, 150.0, 200.0, 100.0, 150.0, 200.0, 100.0, 150.0, 200.0, 100.0, 150.0, 200.0,
            ]
        )
    );

    assert_eq!(record["derived_name"], "derive-base");
    assert_eq!(record["non_parameter_overrides"], json!([]));
    assert_eq!(record["tool_version"], env!("CARGO_PKG_VERSION"));
    let replacements = record["replaced"].as_array().expect("record replacements");
    assert_eq!(replacements.len(), 3);
    for (field_id, role, path) in [
        (SCALAR_ID, SCALAR_ROLE, SCALAR_PATH),
        (LAYER_ID, LAYER_ROLE, LAYER_PATH),
    ] {
        let replacement = replacements
            .iter()
            .find(|replacement| replacement["artifact_role"] == role)
            .unwrap_or_else(|| panic!("record addition for {role}"));
        assert_eq!(replacement["field_ids"], json!([field_id]));
        assert_eq!(replacement["old_sha256"], Value::Null);
        assert_eq!(
            replacement["new_sha256"],
            digest_and_size(&out.join(path)).0
        );
    }
    let removal = replacements
        .iter()
        .find(|replacement| replacement["artifact_role"] == "parameter.scalars")
        .expect("scalar removal record");
    assert_eq!(removal["field_ids"], json!([SCALAR_ID, LAYER_ID]));
    assert_eq!(removal["old_sha256"], SCALARS_DIGEST);
    assert_eq!(removal["new_sha256"], Value::Null);

    assert_valid_package(&out);
    assert_eq!(describe(&base)["content_hash"], record["base_content_hash"]);
    assert_eq!(
        describe(&out)["content_hash"],
        record["derived_content_hash"]
    );
}

#[test]
fn materialize_refuses_descendant_record_path_atomically() {
    let temp = TempDir::new();
    let out = temp.path().join("descendant-out");
    let record = out.join("record.json");
    let output = run_materialize(&materialize_fixture(), &out, &record);
    let out_text = out.to_string_lossy();
    assert_failed_command(&output, &["resolving existing record parent", &out_text]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(record.to_string_lossy().as_ref()),
        "stderr names descendant record: {stderr}"
    );
    assert!(!out.exists());
    assert!(!record.exists());
    assert_no_materialize_temporary_siblings(temp.path());
}

#[test]
fn materialize_refuses_rasters_only_base_atomically() {
    let temp = TempDir::new();
    let first_out = temp.path().join("first-out");
    let first_record = temp.path().join("first-record.json");
    let first = run_materialize(&materialize_fixture(), &first_out, &first_record);
    assert_eq!(
        first.status.code(),
        Some(0),
        "first materialize stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_valid_package(&first_out);

    let second_out = temp.path().join("second-out");
    let second_record = temp.path().join("second-record.json");
    let second = run_materialize(&first_out, &second_out, &second_record);
    assert_failed_command(
        &second,
        &["materialize requires exactly one hmx/parameter_scalars_v1 artifact; found 0"],
    );
    assert_valid_package(&first_out);
    assert!(!second_out.exists());
    assert!(!second_record.exists());
    assert_no_materialize_temporary_siblings(temp.path());
}

#[test]
fn materialize_refuses_existing_destination_without_modification() {
    let temp = TempDir::new();
    let out = temp.path().join("existing-out");
    fs::create_dir(&out).expect("create existing output");
    let sentinel = out.join("sentinel.bin");
    fs::write(&sentinel, b"existing destination sentinel").expect("write sentinel");
    let before = collect_files(&out);
    let record = temp.path().join("existing-record.json");
    let output = run_materialize(&materialize_fixture(), &out, &record);
    assert_failed_command(&output, &["output path already exists:"]);
    assert_eq!(
        fs::read(&sentinel).expect("read sentinel"),
        b"existing destination sentinel"
    );
    assert_eq!(collect_files(&out), before);
    assert!(!out.join("manifest.json").exists());
    assert!(!out.join("parameter/materialized").exists());
    assert!(!record.exists());
    assert_no_materialize_temporary_siblings(temp.path());
}

#[test]
fn materialize_refuses_generated_path_prefix_collision_atomically() {
    const COLLISION: &str = "generated artifact path collision `parameter/materialized/8f9064ae12b0f4fbd785e178faeccef0ab390973a5eb5dd56fa3f380dd7fee4c.tif`";

    let temp = TempDir::new();
    let base = temp.path().join("collision-base");
    copy_tree(&materialize_fixture(), &base);
    let old_path = base.join("parameter/spatial_coefficient.tif");
    let collision_path = base.join("parameter/materialized");
    fs::rename(&old_path, &collision_path).expect("rename collision artifact");
    let manifest_path = base.join("manifest.json");
    let mut manifest = read_json(&manifest_path);
    manifest["artifacts"]
        .as_array_mut()
        .expect("manifest artifacts")
        .iter_mut()
        .find(|artifact| artifact["role"] == "parameter.spatial_coefficient")
        .expect("spatial artifact")["path"] = Value::String("parameter/materialized".into());
    fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("serialize collision manifest"),
    )
    .expect("write collision manifest");
    assert_valid_package(&base);

    let out = temp.path().join("collision-out");
    let record = temp.path().join("collision-record.json");
    let output = run_materialize(&base, &out, &record);
    assert_failed_command(&output, &[COLLISION]);
    assert!(!out.exists());
    assert!(!record.exists());
    assert_no_materialize_temporary_siblings(temp.path());
}
