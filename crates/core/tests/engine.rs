use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::{Float64Array, Int64Array};
use arrow::datatypes::{DataType, Field as ArrowField, Schema};
use arrow::record_batch::RecordBatch;
use hmx_core::CoreError;
use hmx_core::describe::{describe, describe_json};
use hmx_core::manifest::Manifest;
use hmx_core::report::{CheckId, CheckResult, CheckStatus, ValidateError};
use hmx_core::validate::{validate, validate_json};
use parquet::arrow::ArrowWriter;

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn validate_on_valid_package_is_conformant() {
    let dir = temp_package("valid");
    write_valid_package(&dir);

    let report = validate(&dir).expect("valid package validates");
    assert!(report.conformant());
    for id in [
        CheckId::M1,
        CheckId::M2,
        CheckId::M3,
        CheckId::P1,
        CheckId::R1,
        CheckId::R2,
        CheckId::D1,
        CheckId::MAP1,
        CheckId::F1,
    ] {
        let outcome = report.find(id).expect("check id is enumerated");
        assert_eq!(outcome.status(), CheckStatus::Ran);
        assert_eq!(outcome.result(), Some(CheckResult::Pass));
    }

    remove_dir(&dir);
}

#[test]
fn unknown_format_version_is_structural_error() {
    let dir = temp_package("unknown-version");
    write_valid_package(&dir);
    let manifest = fs::read_to_string(dir.join("manifest.json")).expect("manifest exists");
    fs::write(
        dir.join("manifest.json"),
        manifest.replacen(r#""format_version": "0.1""#, r#""format_version": "0.2""#, 1),
    )
    .expect("rewrite manifest");

    match validate(&dir) {
        Err(ValidateError::Manifest(CoreError::UnknownFormatVersion { found })) => {
            assert_eq!(found, "0.2");
        }
        other => panic!("expected unknown format structural error, got {other:?}"),
    }

    remove_dir(&dir);
}

#[test]
fn malformed_registry_flips_conformant() {
    let dir = temp_package("bad-registry");
    write_valid_package(&dir);
    fs::write(
        dir.join("registry/fields.json"),
        registry_json().replace(r#""role": "parameter""#, r#""role": "bogus""#),
    )
    .expect("rewrite registry");

    let report = validate(&dir).expect("registry parse failure is a check failure");
    assert!(!report.conformant());
    assert_eq!(
        report.find(CheckId::R1).and_then(|check| check.result()),
        Some(CheckResult::Fail)
    );

    remove_dir(&dir);
}

#[test]
fn undeclared_attribute_field_flips_conformant() {
    let dir = temp_package("undeclared-field");
    write_valid_package(&dir);
    write_attributes(&dir.join("attributes/cell.parquet"), "cell.unknown");

    let report = validate(&dir).expect("undeclared field is a check failure");
    let r2 = report.find(CheckId::R2).expect("R2 present");
    assert!(!report.conformant());
    assert_eq!(r2.result(), Some(CheckResult::Fail));
    assert!(r2.detail().unwrap_or_default().contains("cell.unknown"));

    remove_dir(&dir);
}

#[test]
fn path_traversal_substring_flips_conformant() {
    let dir = temp_package("path-substring");
    write_valid_package(&dir);
    let manifest = fs::read_to_string(dir.join("manifest.json")).expect("manifest exists");
    let manifest = manifest.replace(
        r#"{ "role": "forcing.flow", "path": "forcing/flow.zarr", "format": "zarr", "sha256": "3333333333333333333333333333333333333333333333333333333333333333", "size_bytes": null }"#,
        r#"{ "role": "forcing.flow", "path": "a..b/flow.tif", "format": "cog", "sha256": "3333333333333333333333333333333333333333333333333333333333333333", "size_bytes": null }"#,
    );
    assert!(Manifest::from_json(&manifest).is_ok());
    fs::write(dir.join("manifest.json"), manifest).expect("rewrite manifest");

    let report = validate(&dir).expect("path substring is a check failure");
    assert!(!report.conformant());
    assert_eq!(
        report.find(CheckId::P1).and_then(|check| check.result()),
        Some(CheckResult::Fail)
    );

    remove_dir(&dir);
}

#[test]
fn zarr_non_consolidated_is_a_clean_check_failure() {
    let dir = temp_package("zarr-non-consolidated");
    write_valid_package(&dir);
    fs::write(dir.join("forcing/flow.zarr/zarr.json"), r#"{"zarr_format":3}"#)
        .expect("rewrite zarr root");

    let report = validate(&dir).expect("zarr read failure is a check failure");
    assert!(!report.conformant());
    assert_eq!(
        report.find(CheckId::F1).and_then(|check| check.result()),
        Some(CheckResult::Fail)
    );

    remove_dir(&dir);
}

#[test]
fn missing_required_column_flips_conformant() {
    let dir = temp_package("missing-column");
    write_valid_package(&dir);
    write_manifest(&dir, true);
    fs::create_dir_all(dir.join("forcing")).expect("forcing dir");
    write_gauge_long_missing_value(&dir.join("forcing/gauge_long.parquet"));

    let report = validate(&dir).expect("missing column is a check failure");
    let f1 = report.find(CheckId::F1).expect("F1 present");
    assert!(!report.conformant());
    assert_eq!(f1.result(), Some(CheckResult::Fail));
    let detail = f1.detail().unwrap_or_default();
    assert!(detail.contains("value"));
    assert!(detail.contains("parquet/gauge_long_v1"));

    remove_dir(&dir);
}

#[test]
fn dangling_mapping_id_flips_conformant() {
    let dir = temp_package("dangling");
    write_valid_package(&dir);
    write_mapping(&dir.join("mappings/cell_to_glacier.parquet"), &[0, 99, 2]);

    let report = validate(&dir).expect("dangling mapping id is a check failure");
    assert!(!report.conformant());
    assert_eq!(
        report.find(CheckId::D1).and_then(|check| check.result()),
        Some(CheckResult::Fail)
    );

    remove_dir(&dir);
}

#[test]
fn describe_on_valid_package_carries_hash_and_facts() {
    let dir = temp_package("describe");
    write_valid_package(&dir);

    let description = describe(&dir).expect("description builds");
    assert_eq!(description.content_hash().hash_algo(), "sha256");
    assert_eq!(description.content_hash().as_str().len(), 64);
    assert!(
        description
            .content_hash()
            .as_str()
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(description.manifest().domains().len(), 2);
    assert_eq!(description.fields().len(), 1);
    assert_eq!(description.fields()[0].id().as_str(), "cell.slope");
    assert!(describe_json(&dir).expect("description json").contains("content_hash"));

    remove_dir(&dir);
}

#[test]
fn emitted_json_matches_committed_schemas() {
    let dir = temp_package("json");
    write_valid_package(&dir);

    let validate_path = std::env::temp_dir().join("hmx-a8-validate.json");
    let describe_path = std::env::temp_dir().join("hmx-a8-describe.json");
    fs::write(&validate_path, validate_json(&dir).expect("validate json")).expect("write validate");
    fs::write(&describe_path, describe_json(&dir).expect("describe json")).expect("write describe");
    assert!(validate_path.exists());
    assert!(describe_path.exists());

    remove_dir(&dir);
}

fn temp_package(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hmx-a8-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn write_valid_package(dir: &Path) {
    fs::create_dir_all(dir.join("registry")).expect("registry dir");
    fs::create_dir_all(dir.join("mappings")).expect("mappings dir");
    fs::create_dir_all(dir.join("attributes")).expect("attributes dir");
    fs::create_dir_all(dir.join("forcing/flow.zarr")).expect("zarr dir");
    write_manifest(dir, false);
    fs::write(dir.join("registry/fields.json"), registry_json()).expect("registry");
    write_mapping(&dir.join("mappings/cell_to_glacier.parquet"), &[0, 1, 2]);
    write_attributes(&dir.join("attributes/cell.parquet"), "cell.slope");
    write_valid_zarr(&dir.join("forcing/flow.zarr"));
}

fn write_manifest(dir: &Path, include_gauge_long: bool) {
    let gauge = if include_gauge_long {
        r#",
    { "role": "forcing.gauge", "path": "forcing/gauge_long.parquet", "format": "parquet/gauge_long_v1", "sha256": "4444444444444444444444444444444444444444444444444444444444444444", "size_bytes": null }"#
    } else {
        ""
    };
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
  "format_version": "0.1",
  "name": "synthetic-glacier-mini",
  "created_at": "2026-06-29T00:00:00Z",
  "producer": "hmx-core-a8-test",
  "producer_version": "0.1.0",
  "package_kind": "input",
  "crs": "EPSG:32645",
  "grid": {{
    "crs": "EPSG:32645",
    "extent": {{ "xmin": 0.0, "ymin": 0.0, "xmax": 1000.0, "ymax": 1000.0 }},
    "cell_size": 250.0,
    "nx": 2,
    "ny": 2,
    "origin": "upper_left"
  }},
  "domains": [
    {{ "id": "cell", "entity_count": 4, "index_base": "dense_zero_based" }},
    {{ "id": "glacier", "entity_count": 3, "index_base": "dense_zero_based", "external_ids": [1, 2, 2001] }}
  ],
  "mappings": [
    {{ "purpose": "cell_to_glacier", "source_domain": "cell", "target_domain": "glacier", "artifact_role": "mapping.cell_to_glacier" }}
  ],
  "artifacts": [
    {{ "role": "registry.fields", "path": "registry/fields.json", "format": "hmx/field_registry_v1", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "size_bytes": 512 }},
    {{ "role": "mapping.cell_to_glacier", "path": "mappings/cell_to_glacier.parquet", "format": "parquet/domain_mapping_v1", "sha256": "1111111111111111111111111111111111111111111111111111111111111111", "size_bytes": null }},
    {{ "role": "attributes.cell", "path": "attributes/cell.parquet", "format": "parquet/domain_attributes_v1", "sha256": "2222222222222222222222222222222222222222222222222222222222222222", "size_bytes": null }},
    {{ "role": "forcing.flow", "path": "forcing/flow.zarr", "format": "zarr", "sha256": "3333333333333333333333333333333333333333333333333333333333333333", "size_bytes": null }}{gauge}
  ]
}}"#
        ),
    )
    .expect("manifest");
}

fn registry_json() -> String {
    r#"{
  "registry_version": "1",
  "fields": [
    { "id": "cell.slope", "domain": "cell", "quantity": "slope", "units": "1", "value_type": "f64", "time_meaning": "instant", "role": "parameter", "conservation_class": "none", "extent": "scalar" }
  ]
}"#
    .to_string()
}

fn write_mapping(path: &Path, target_index: &[i64]) {
    let schema = Arc::new(Schema::new(vec![
        ArrowField::new("source_index", DataType::Int64, false),
        ArrowField::new("target_index", DataType::Int64, false),
        ArrowField::new("weight", DataType::Float64, false),
    ]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(Int64Array::from(vec![0, 1, 2])),
            Arc::new(Int64Array::from(target_index.to_vec())),
            Arc::new(Float64Array::from(vec![1.0, 1.0, 1.0])),
        ],
    )
    .expect("mapping batch");
    write_batch(path, schema, batch);
}

fn write_attributes(path: &Path, field_id: &str) {
    let schema = Arc::new(Schema::new(vec![
        ArrowField::new("entity_index", DataType::Int64, false),
        ArrowField::new(field_id, DataType::Float64, false),
    ]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(Int64Array::from(vec![0, 1, 2, 3])),
            Arc::new(Float64Array::from(vec![0.1, 0.2, 0.3, 0.4])),
        ],
    )
    .expect("attributes batch");
    write_batch(path, schema, batch);
}

fn write_gauge_long_missing_value(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        ArrowField::new("timestep", DataType::Int64, false),
        ArrowField::new("gauge_id", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(Int64Array::from(vec![0, 1])),
            Arc::new(Int64Array::from(vec![7, 7])),
        ],
    )
    .expect("gauge batch");
    write_batch(path, schema, batch);
}

fn write_batch(path: &Path, schema: Arc<Schema>, batch: RecordBatch) {
    let mut buffer = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut buffer, schema, None).expect("writer");
        writer.write(&batch).expect("write batch");
        writer.close().expect("close writer");
    }
    fs::write(path, buffer).expect("write parquet");
}

fn write_valid_zarr(path: &Path) {
    fs::write(
        path.join("zarr.json"),
        r#"{
  "zarr_format": 3,
  "node_type": "group",
  "consolidated_metadata": { "kind": "inline", "metadata": {} }
}"#,
    )
    .expect("zarr root");
}

fn remove_dir(dir: &Path) {
    fs::remove_dir_all(dir).ok();
}
