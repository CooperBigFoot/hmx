//! `hmx-core` — Hydrology Model Exchange core.
//!
//! Module map: [`types`] carries inert domain values, `dto` carries the private
//! serde layer, and [`manifest`] exposes the parse boundary and `manifest.json`
//! reader. Payload readers, verbs, and the package content-hash land in later
//! steps.

use tracing::debug;

pub mod manifest;
pub mod readers;
pub mod types;

mod dto;

/// The compile-time version of `hmx-core` (its `CARGO_PKG_VERSION`).
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the `hmx-core` crate version.
pub fn core_version() -> &'static str {
    debug!("hmx-core core_version() called");
    CORE_VERSION
}

/// The crate-wide error for the `hmx-core` boundary parse (manifest reader).
///
/// Every variant is a manifest-LOCAL failure raised at the parse boundary
/// (parse, don't validate). Cross-file conformance outcomes (field-registry
/// presence, mapping artifact_role resolution, per-format column shapes,
/// external_ids length cross-check, numeric range checks) are NOT here — they
/// are reported by the A8 `validate` verb, not raised by the reader.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// `format_version` is not the single recognized value `"0.1"` (spec §0 hard
    /// cut). Read FIRST, so this wins over every other field-value error.
    #[error("unknown HMX format_version {found:?}: the only recognized value is \"0.1\" (spec §0 hard cut)")]
    UnknownFormatVersion {
        /// The rejected raw `format_version` string, echoed verbatim.
        found: String,
    },
    /// A closed-set value (package_kind, index_base, grid origin, mapping
    /// purpose, artifact format, artifact time_meaning) is not in its enum.
    #[error("invalid value {found:?} for closed-set field {field}")]
    InvalidEnumValue {
        /// The field whose value was rejected (e.g. `"package_kind"`).
        field: &'static str,
        /// The rejected raw string.
        found: String,
    },
    /// A required manifest field is absent (M3 too-few; serde `missing field`).
    #[error("manifest is missing required field `{field}`")]
    MissingManifestField {
        /// The absent field name extracted from the serde error.
        field: String,
    },
    /// A key beyond the schema's `additionalProperties:false` set is present
    /// (M3 too-many; serde `unknown field`). Catches a stray `glacier_count` etc.
    #[error("manifest carries an unexpected field `{field}` (additionalProperties:false)")]
    ExtraManifestField {
        /// The offending extra field name.
        field: String,
    },
    /// The manifest is not valid JSON, or a value has the wrong JSON type.
    #[error("manifest JSON could not be parsed: {detail}")]
    InvalidManifestJson {
        /// The raw serde_json error message.
        detail: String,
    },
    /// A required non-empty string field is empty (spec §3.4/§4 — e.g. an empty
    /// `crs`, the F1 guard).
    #[error("required field {field} must be a non-empty string")]
    EmptyField {
        /// The empty field name.
        field: &'static str,
    },
    /// `created_at` is not a strict RFC 3339 date-time (spec §3.4).
    #[error("created_at {value:?} is not a strict RFC 3339 date-time")]
    InvalidTimestamp {
        /// The rejected timestamp string.
        value: String,
    },
    /// An artifact `sha256` is not 64 lowercase-hex characters (spec §7.2).
    #[error("artifact sha256 {value:?} is not 64 lowercase-hex characters")]
    InvalidSha256 {
        /// The rejected sha256 string.
        value: String,
    },
    /// An artifact `path` is not package-relative (absolute, parent-traversal,
    /// or empty — spec §2.2, prevents F1-class absolute-path leakage).
    #[error("artifact path {path:?} is not package-relative ({reason})")]
    InvalidArtifactPath {
        /// The rejected path.
        path: String,
        /// Why it was rejected: `"absolute"`, `"parent traversal"`, or `"empty"`.
        reason: &'static str,
    },
    /// A `cell_to_gauge` mapping omits its required `variable` (spec §8.2).
    #[error("a cell_to_gauge mapping must declare `variable` (spec §8.2)")]
    MappingMissingVariable,
    /// `manifest.json` could not be read from the package root (`manifest::read`).
    #[error("could not read manifest at {path}: {detail}")]
    ManifestUnreadable {
        /// The attempted `<package_root>/manifest.json` path.
        path: String,
        /// The underlying IO error message.
        detail: String,
    },
    /// An artifact file could not be read from disk (the BULK readers open the
    /// declared `path` from the package root). Distinct from `ManifestUnreadable`,
    /// which is the manifest JSON itself.
    #[error("could not read artifact at {path}: {detail}")]
    ArtifactUnreadable {
        /// The artifact path the reader attempted to open.
        path: String,
        /// The underlying IO error message.
        detail: String,
    },
    /// A parquet (or GeoParquet) footer/schema failed to decode (corrupt file,
    /// not a parquet file, or unreadable metadata). The reader decodes NO data page.
    #[error("could not read parquet metadata for {artifact}: {detail}")]
    ParquetRead {
        /// The artifact path/label echoed for diagnostics.
        artifact: String,
        /// The underlying parquet/arrow reader error.
        detail: String,
    },
    /// A GeoParquet `geo` footer block was present but is not valid JSON / lacks the
    /// expected keys. Absence of the `geo` key is NOT this error — that is surfaced
    /// as "no geo metadata", not a failure.
    #[error("could not parse GeoParquet `geo` metadata for {artifact}: {detail}")]
    GeoMetadataMalformed {
        /// The artifact path/label echoed for diagnostics.
        artifact: String,
        /// What was wrong with the `geo` JSON block.
        detail: String,
    },
    /// A COG GeoTIFF could not be opened or its tags could not be read. The reader
    /// reads georef/band TAGS only and decodes NO pixel strip/tile.
    #[error("could not read COG metadata for {artifact}: {detail}")]
    CogRead {
        /// The artifact path/label echoed for diagnostics.
        artifact: String,
        /// The underlying `tiff` decoder error.
        detail: String,
    },
    /// A Zarr v3 `zarr.json` (or a 1-D coordinate chunk) could not be read/decoded.
    /// The reader reads `zarr.json` + 1-D coordinate chunks only — never a data chunk.
    #[error("could not read Zarr metadata for {artifact}: {detail}")]
    ZarrRead {
        /// The artifact path/label echoed for diagnostics.
        artifact: String,
        /// The underlying zarr-metadata / decode error.
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use crate::core_version;

    #[test]
    fn core_version_is_non_empty() {
        assert!(!core_version().is_empty());
    }
}
