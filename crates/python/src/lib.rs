//! `hmx` — PyO3 binding (SKELETON).
//!
//! A1 exposes a single import/link proof, `__core_version`, which returns
//! `hmx-core`'s version. The real `validate` / `describe` mirrors and the typed
//! exception hierarchy (with the hard version-cut never softened to a report) land
//! in A10. This crate carries **zero** contract logic.
//!
//! The PyO3 `extension-module` feature is optional and non-default (see
//! `Cargo.toml`): `cargo build/test/clippy` link with it OFF so the `rlib`
//! unit-test target builds on macOS; `maturin` enables it for the shipped wheel.

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// Return the `hmx-core` version — an import/link proof until A10 adds the verbs.
///
/// Touches no IO and no contract logic, so a successful
/// `import hmx; hmx.__core_version()` confirms the abi3 extension links against
/// `hmx-core` and imports under the host interpreter.
#[pyfunction]
fn __core_version() -> &'static str {
    hmx_core::core_version()
}

/// The `hmx` Python module entry point. Registers only the A1 link proof.
#[pymodule]
fn hmx(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(__core_version, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::__core_version;

    /// Builds against the `rlib` target with `extension-module` OFF — the macOS
    /// Rust-level link proof.
    #[test]
    fn core_version_is_non_empty() {
        assert!(!__core_version().is_empty());
    }
}
