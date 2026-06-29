//! `hmx-core` — Hydrology Model Exchange core (SKELETON).
//!
//! This crate will hold all HMX contract logic: the typed manifest / field
//! registry / cross-domain mapping / multi-entity domain model, the metadata-only
//! readers, the manifest content-hash, and the `validate` / `describe` verbs.
//! **None of that exists yet** — this is the M13/A1 compiling skeleton. Real types
//! and readers land in A3+; the normative contract is authored in
//! `spec/HMX_SPEC.md` (step A2).
//!
//! The only public surface today is a version/link proof so the `hmx` binary and
//! the `hmx-python` binding can depend on `hmx-core` and build.

use tracing::debug;

/// The compile-time version of `hmx-core` (its `CARGO_PKG_VERSION`).
///
/// A skeleton link/version proof until the real verbs land in A3+.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the `hmx-core` crate version.
///
/// Placeholder link proof: touches no IO and no contract logic. Real verbs
/// (`validate` / `describe`) land in A8/A9; readers in A3/A4.
pub fn core_version() -> &'static str {
    debug!("hmx-core skeleton core_version() called");
    CORE_VERSION
}

/// The crate-wide error type (SKELETON).
///
/// Real variants — manifest parse, the format-version hard cut, reader faults,
/// validation failures — land in A3+. The single placeholder variant exists only
/// so the error surface compiles before A3 fills it in.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Fires from any not-yet-implemented `hmx-core` entry point in the A1 skeleton.
    #[error("hmx-core is not yet implemented (A1 skeleton; real logic lands in A3+)")]
    NotYetImplemented,
}

#[cfg(test)]
mod tests {
    use super::{CoreError, core_version};

    #[test]
    fn core_version_is_non_empty() {
        assert!(!core_version().is_empty());
    }

    #[test]
    fn not_yet_implemented_displays() {
        assert!(
            CoreError::NotYetImplemented
                .to_string()
                .contains("not yet implemented")
        );
    }
}
