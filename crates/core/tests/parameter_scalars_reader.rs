use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hmx_core::CoreError;
use hmx_core::readers::parameter_scalars_reader::{ParameterScalarValue, read_parameter_scalars};
use hmx_core::registry::FieldRegistry;
use hmx_core::types::{Extent, FieldId, SemanticRole};

static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

const REGISTRY_JSON: &str = r#"{
  "registry_version": "1",
  "fields": [
    {"id":"cells.scalar_parameter","domain":"cell","quantity":"coefficient","units":"1","value_type":"f64","time_meaning":"instant","role":"parameter","conservation_class":"none","extent":"scalar"},
    {"id":"cells.layer_parameter","domain":"cell","quantity":"capacity","units":"mm","value_type":"f64","time_meaning":"instant","role":"parameter","conservation_class":"none","extent":"per_layer","layer_count":3},
    {"id":"cells.diagnostic","domain":"cell","quantity":"diagnostic","units":"1","value_type":"f64","time_meaning":"instant","role":"diagnostic","conservation_class":"none","extent":"scalar"}
  ]
}"#;

#[test]
fn reads_typed_scalar_and_per_layer_values() {
    let registry = registry();
    let file =
        TempJson::new(r#"{"cells.layer_parameter":[1.0,2.0,3.0],"cells.scalar_parameter":4.5}"#);

    let values = read_parameter_scalars(file.path(), &registry)
        .expect("valid parameter scalars should parse");

    assert_eq!(values.len(), 2);
    assert!(!values.is_empty());
    assert_eq!(
        values
            .get(&FieldId::new("cells.scalar_parameter"))
            .and_then(ParameterScalarValue::as_scalar),
        Some(4.5)
    );
    assert_eq!(
        values
            .get(&FieldId::new("cells.layer_parameter"))
            .and_then(ParameterScalarValue::as_per_layer),
        Some([1.0, 2.0, 3.0].as_slice())
    );
    assert_eq!(
        values.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["cells.layer_parameter", "cells.scalar_parameter"]
    );
    assert!(matches!(
        values.get(&FieldId::new("cells.scalar_parameter")),
        Some(ParameterScalarValue::Scalar { value }) if *value == 4.5
    ));
}

#[test]
fn rejects_invalid_json_shapes_and_numbers() {
    let registry = registry();
    for json in [
        "{not json}",
        "{}",
        r#"{"cells.scalar_parameter":"4.5"}"#,
        r#"{"cells.layer_parameter":[[1.0],2.0,3.0]}"#,
        r#"{"cells.scalar_parameter":1e400}"#,
    ] {
        let file = TempJson::new(json);
        assert!(matches!(
            read_parameter_scalars(file.path(), &registry),
            Err(CoreError::InvalidParameterScalarsJson { .. })
        ));
    }
}

#[test]
fn resolves_keys_by_exact_field_id() {
    let registry = registry();
    let file = TempJson::new(r#"{"scalar_parameter":4.5}"#);

    match read_parameter_scalars(file.path(), &registry).unwrap_err() {
        CoreError::UndeclaredField { id } => assert_eq!(id, "scalar_parameter"),
        other => panic!("expected UndeclaredField, got {other:?}"),
    }
}

#[test]
fn rejects_non_parameter_registry_fields() {
    let registry = registry();
    let file = TempJson::new(r#"{"cells.diagnostic":1.0}"#);

    match read_parameter_scalars(file.path(), &registry).unwrap_err() {
        CoreError::ParameterScalarRole { id, actual } => {
            assert_eq!(id, "cells.diagnostic");
            assert_eq!(actual, SemanticRole::Diagnostic);
        }
        other => panic!("expected ParameterScalarRole, got {other:?}"),
    }
}

#[test]
fn rejects_extent_shape_mismatches() {
    let registry = registry();
    for (json, id, expected, observed) in [
        (
            r#"{"cells.scalar_parameter":[1.0]}"#,
            "cells.scalar_parameter",
            Extent::Scalar,
            "array",
        ),
        (
            r#"{"cells.layer_parameter":1.0}"#,
            "cells.layer_parameter",
            Extent::PerLayer,
            "number",
        ),
    ] {
        let file = TempJson::new(json);
        match read_parameter_scalars(file.path(), &registry).unwrap_err() {
            CoreError::ParameterScalarExtent {
                id: actual_id,
                expected: actual_expected,
                observed: actual_observed,
            } => {
                assert_eq!(actual_id, id);
                assert_eq!(actual_expected, expected);
                assert_eq!(actual_observed, observed);
            }
            other => panic!("expected ParameterScalarExtent, got {other:?}"),
        }
    }
}

#[test]
fn rejects_wrong_layer_cardinality() {
    let registry = registry();
    for (json, actual) in [
        (r#"{"cells.layer_parameter":[]}"#, 0),
        (r#"{"cells.layer_parameter":[1.0,2.0]}"#, 2),
        (r#"{"cells.layer_parameter":[1.0,2.0,3.0,4.0]}"#, 4),
    ] {
        let file = TempJson::new(json);
        match read_parameter_scalars(file.path(), &registry).unwrap_err() {
            CoreError::ParameterScalarLayerCount {
                id,
                expected,
                actual: actual_count,
            } => {
                assert_eq!(id, "cells.layer_parameter");
                assert_eq!(expected, 3);
                assert_eq!(actual_count, actual);
            }
            other => panic!("expected ParameterScalarLayerCount, got {other:?}"),
        }
    }
}

#[test]
fn unreadable_path_is_fallible() {
    let registry = registry();
    let path = unique_path();

    match read_parameter_scalars(&path, &registry).unwrap_err() {
        CoreError::ArtifactUnreadable {
            path: actual_path, ..
        } => assert_eq!(actual_path, path.display().to_string()),
        other => panic!("expected ArtifactUnreadable, got {other:?}"),
    }
}

fn registry() -> FieldRegistry {
    FieldRegistry::from_json(REGISTRY_JSON).expect("test registry should parse")
}

struct TempJson {
    path: PathBuf,
}

impl TempJson {
    fn new(contents: &str) -> Self {
        let path = unique_path();
        fs::write(&path, contents).expect("temporary JSON should be writable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempJson {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unique_path() -> PathBuf {
    let id = NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hmx-parameter-scalars-{}-{id}.json",
        std::process::id()
    ))
}
