//! `hmx` — a thin PyO3 mirror of the `hmx-core` contract verbs.
//!
//! This crate adds **zero** contract logic: it exposes `hmx-core`'s `validate` /
//! `describe` verbs to Python. Each function calls the matching `hmx-core`
//! `*_json` verb, parses the **already-produced** JSON string into a Python `dict`
//! via `json.loads`, and maps the typed boundary errors to Python exceptions. No
//! check rule, no manifest parse, no reader lives here — all contract logic is in
//! `hmx-core`.
//!
//! ## The §0 hard cut is preserved through the binding
//!
//! A wrong `format_version` surfaces from `hmx-core` as
//! `...::Manifest(CoreError::UnknownFormatVersion { .. })` — an `Err`, never a
//! softened `conformant: false` report. The binding maps exactly that variant to
//! a dedicated [`UnknownFormatVersionError`]; every other boundary error maps to
//! the [`HmxError`] base.
//!
//! The PyO3 `extension-module` feature is optional and non-default (see
//! `Cargo.toml`): `cargo build/test/clippy` link with it OFF so the `rlib`
//! unit-test target builds on macOS; `maturin` enables it for the shipped wheel.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use hmx_core::CoreError;
use hmx_core::describe::describe_json;
use hmx_core::report::{DescribeError, ValidateError};
use hmx_core::validate::validate_json;

create_exception!(
    hmx,
    HmxError,
    PyException,
    "Base for every error raised by the hmx binding (a structural / entry failure from hmx-core)."
);

create_exception!(
    hmx,
    UnknownFormatVersionError,
    HmxError,
    "The §0 hard cut: manifest.format_version is not the single supported version. Never softened into a conformant:false report."
);

/// Which Python exception type a boundary error maps to.
///
/// The §0 hard version cut is the one load-bearing distinction: it selects
/// [`HmxExceptionKind::UnknownFormatVersion`] and never becomes a report. Every
/// other structural / entry failure is [`HmxExceptionKind::General`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HmxExceptionKind {
    UnknownFormatVersion,
    General,
}

impl HmxExceptionKind {
    fn from_validate_error(err: &ValidateError) -> Self {
        match err {
            ValidateError::Manifest(CoreError::UnknownFormatVersion { .. }) => {
                Self::UnknownFormatVersion
            }
            _ => Self::General,
        }
    }

    fn from_describe_error(err: &DescribeError) -> Self {
        match err {
            DescribeError::Manifest(CoreError::UnknownFormatVersion { .. }) => {
                Self::UnknownFormatVersion
            }
            _ => Self::General,
        }
    }

    fn into_pyerr(self, message: String) -> PyErr {
        match self {
            Self::UnknownFormatVersion => UnknownFormatVersionError::new_err(message),
            Self::General => HmxError::new_err(message),
        }
    }
}

/// Parses an `hmx-core` `*_json` string into a Python object via `json.loads`.
fn json_string_to_pyobject(py: Python<'_>, json: &str) -> PyResult<Py<PyAny>> {
    let json_module = PyModule::import(py, "json")?;
    let parsed = json_module
        .call_method1("loads", (json,))
        .map_err(|err| HmxError::new_err(format!("failed to parse hmx-core JSON output: {err}")))?;
    Ok(parsed.unbind())
}

/// Validate a package and return the conformance report as a Python `dict`.
#[pyfunction]
fn validate(py: Python<'_>, path: &str) -> PyResult<Py<PyAny>> {
    match validate_json(path) {
        Ok(json) => json_string_to_pyobject(py, &json),
        Err(err) => Err(HmxExceptionKind::from_validate_error(&err).into_pyerr(err.to_string())),
    }
}

/// Describe a package and return the self-description as a Python `dict`.
#[pyfunction]
fn describe(py: Python<'_>, path: &str) -> PyResult<Py<PyAny>> {
    match describe_json(path) {
        Ok(json) => json_string_to_pyobject(py, &json),
        Err(err) => Err(HmxExceptionKind::from_describe_error(&err).into_pyerr(err.to_string())),
    }
}

/// Return the `hmx-core` version — an import/link proof.
#[pyfunction]
fn __core_version() -> &'static str {
    hmx_core::core_version()
}

/// The `hmx` Python module entry point.
#[pymodule]
fn hmx(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("HmxError", m.py().get_type::<HmxError>())?;
    m.add(
        "UnknownFormatVersionError",
        m.py().get_type::<UnknownFormatVersionError>(),
    )?;
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(describe, m)?)?;
    m.add_function(wrap_pyfunction!(__core_version, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use hmx_core::CoreError;
    use hmx_core::report::{DescribeError, ValidateError};

    use super::{__core_version, HmxExceptionKind};

    #[test]
    fn core_version_is_non_empty() {
        assert!(!__core_version().is_empty());
    }

    #[test]
    fn validate_unknown_format_version_maps_to_hard_cut_kind() {
        let err = ValidateError::Manifest(CoreError::UnknownFormatVersion {
            found: "0.2".to_string(),
        });
        assert_eq!(
            HmxExceptionKind::from_validate_error(&err),
            HmxExceptionKind::UnknownFormatVersion,
        );
    }

    #[test]
    fn describe_unknown_format_version_maps_to_hard_cut_kind() {
        let err = DescribeError::Manifest(CoreError::UnknownFormatVersion {
            found: "9.9".to_string(),
        });
        assert_eq!(
            HmxExceptionKind::from_describe_error(&err),
            HmxExceptionKind::UnknownFormatVersion,
        );
    }

    #[test]
    fn validate_manifest_unreadable_maps_to_general() {
        let err = ValidateError::ManifestUnreadable {
            path: "/no/such/pkg/manifest.json".to_string(),
            detail: "No such file or directory".to_string(),
        };
        assert_eq!(
            HmxExceptionKind::from_validate_error(&err),
            HmxExceptionKind::General,
        );
    }

    #[test]
    fn validate_other_manifest_error_maps_to_general() {
        let err = ValidateError::Manifest(CoreError::MissingManifestField {
            field: "crs".to_string(),
        });
        assert_eq!(
            HmxExceptionKind::from_validate_error(&err),
            HmxExceptionKind::General,
        );
    }

    #[test]
    fn describe_registry_unreadable_maps_to_general() {
        let err = DescribeError::Registry {
            path: "/no/such/pkg/registry/fields.json".to_string(),
            detail: "No such file or directory".to_string(),
        };
        assert_eq!(
            HmxExceptionKind::from_describe_error(&err),
            HmxExceptionKind::General,
        );
    }
}
