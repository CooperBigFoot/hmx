//! Frontend-agnostic `validate` engine.
//!
//! A violated HMX 0.1 MUST that runs is recorded as a `ran:fail`
//! [`CheckOutcome`](crate::report::CheckOutcome) and makes the report
//! `conformant:false`. [`ValidateError`] is reserved for structural entry
//! failures: unreadable manifest, the format-version hard cut, or malformed
//! manifest JSON. The staged order is fixed: read manifest bytes, parse the
//! manifest, compose the closed checks, then derive the report.
//!
//! OD6 decisions: all checks are error-severity MUST checks; core has no
//! `--strict`; the wire schema keeps `id` as a free string with the Rust
//! [`CheckId`] enum as the closed set; every HMX 0.1 check is `metadata_deep`.

use std::path::Path;

use tracing::{info, instrument};

use crate::domains::{DomainDescriptor, cross_check};
use crate::manifest::Manifest;
use crate::mappings::MappingGeometry;
use crate::readers::cog_reader::read_cog_metadata;
use crate::readers::control_plane::read_domain_attributes;
use crate::readers::geoparquet_reader::read_geoparquet_metadata;
use crate::readers::parquet_meta::read_parquet_metadata;
use crate::readers::zarr_reader::read_zarr_metadata;
use crate::registry::FieldRegistry;
use crate::report::{
    ALL_CHECK_IDS, CheckId, CheckOutcome, DepthClass, ValidateError, ValidationReport,
};
use crate::types::{Artifact, ArtifactFormat, ArtifactRole, FieldId};

#[instrument(fields(package_root = %package_root.as_ref().display()))]
pub fn validate(package_root: impl AsRef<Path>) -> Result<ValidationReport, ValidateError> {
    let package_root = package_root.as_ref();
    let manifest_path = package_root.join("manifest.json");
    let json =
        std::fs::read_to_string(&manifest_path).map_err(|e| ValidateError::ManifestUnreadable {
            path: manifest_path.display().to_string(),
            detail: e.to_string(),
        })?;
    let manifest = Manifest::from_json(&json).map_err(ValidateError::Manifest)?;
    let registry = load_registry_for_validation(package_root, &manifest);

    let outcomes = ALL_CHECK_IDS
        .into_iter()
        .map(|id| run_check(id, package_root, &manifest, registry.as_ref()))
        .collect();
    info!("assembled validation report");
    Ok(ValidationReport::from_outcomes(outcomes))
}

#[instrument(fields(package_root = %package_root.as_ref().display()))]
pub fn validate_json(package_root: impl AsRef<Path>) -> Result<String, ValidateError> {
    validate(package_root)?
        .to_json_string()
        .map_err(|e| ValidateError::Serialize {
            detail: e.to_string(),
        })
}

fn run_check(
    id: CheckId,
    package_root: &Path,
    manifest: &Manifest,
    registry: Option<&Result<FieldRegistry, String>>,
) -> CheckOutcome {
    match id {
        CheckId::M1 | CheckId::M2 | CheckId::M3 => {
            CheckOutcome::ran_pass(id, DepthClass::MetadataDeep)
        }
        CheckId::P1 => check_p1(manifest),
        CheckId::R1 => check_r1(registry),
        CheckId::R2 => check_r2(package_root, manifest, registry),
        CheckId::D1 => check_d1(package_root, manifest),
        CheckId::MAP1 => check_map1(manifest),
        CheckId::F1 => check_f1(package_root, manifest),
    }
}

fn check_p1(manifest: &Manifest) -> CheckOutcome {
    if let Some(artifact) = manifest.artifacts().iter().find(|artifact| {
        let path = artifact.path.as_str();
        path.is_empty() || path.starts_with('/') || path.contains("..")
    }) {
        return CheckOutcome::ran_fail(
            CheckId::P1,
            DepthClass::MetadataDeep,
            format!(
                "artifact path {:?} violates package-relative rule",
                artifact.path.as_str()
            ),
        );
    }
    CheckOutcome::ran_pass(CheckId::P1, DepthClass::MetadataDeep)
}

fn check_r1(registry: Option<&Result<FieldRegistry, String>>) -> CheckOutcome {
    match registry {
        Some(Ok(_)) => CheckOutcome::ran_pass(CheckId::R1, DepthClass::MetadataDeep),
        Some(Err(detail)) => {
            CheckOutcome::ran_fail(CheckId::R1, DepthClass::MetadataDeep, detail.clone())
        }
        None => CheckOutcome::ran_fail(
            CheckId::R1,
            DepthClass::MetadataDeep,
            "no registry load result was produced",
        ),
    }
}

fn check_r2(
    package_root: &Path,
    manifest: &Manifest,
    registry: Option<&Result<FieldRegistry, String>>,
) -> CheckOutcome {
    let registry = match registry {
        Some(Ok(registry)) => registry,
        Some(Err(_)) | None => {
            return CheckOutcome::skipped(
                CheckId::R2,
                DepthClass::MetadataDeep,
                "R1 failed; no parsed registry is available",
            );
        }
    };

    for artifact in manifest
        .artifacts()
        .iter()
        .filter(|artifact| artifact.format == ArtifactFormat::ParquetDomainAttributesV1)
    {
        let path = package_root.join(artifact.path.as_str());
        let attributes = match read_domain_attributes(&path) {
            Ok(attributes) => attributes,
            Err(err) => {
                return CheckOutcome::ran_fail(
                    CheckId::R2,
                    DepthClass::MetadataDeep,
                    format!("{}: {err}", artifact.path.as_str()),
                );
            }
        };
        for column in attributes.attributes() {
            let field_id = FieldId::new(column.field_id());
            if let Err(err) = registry.require(&field_id) {
                return CheckOutcome::ran_fail(
                    CheckId::R2,
                    DepthClass::MetadataDeep,
                    err.to_string(),
                );
            }
        }
    }
    CheckOutcome::ran_pass(CheckId::R2, DepthClass::MetadataDeep)
}

fn check_d1(package_root: &Path, manifest: &Manifest) -> CheckOutcome {
    for domain in manifest.domains() {
        let descriptor = match DomainDescriptor::from_domain(domain) {
            Ok(descriptor) => descriptor,
            Err(err) => {
                return CheckOutcome::ran_fail(
                    CheckId::D1,
                    DepthClass::MetadataDeep,
                    err.to_string(),
                );
            }
        };
        if domain.external_ids.is_none() {
            continue;
        }
        for mapping in manifest.mappings().iter().filter(|mapping| {
            mapping.source_domain == domain.id || mapping.target_domain == domain.id
        }) {
            let Some(artifact) = artifact_for_role(manifest, &mapping.artifact_role) else {
                continue;
            };
            let path = package_root.join(artifact.path.as_str());
            let geometry = match MappingGeometry::read(&path, mapping) {
                Ok(geometry) => geometry,
                Err(err) => {
                    return CheckOutcome::ran_fail(
                        CheckId::D1,
                        DepthClass::MetadataDeep,
                        format!("{}: {err}", artifact.path.as_str()),
                    );
                }
            };
            if let Err(err) = cross_check(&descriptor, &geometry) {
                return CheckOutcome::ran_fail(
                    CheckId::D1,
                    DepthClass::MetadataDeep,
                    err.to_string(),
                );
            }
        }
    }
    CheckOutcome::ran_pass(CheckId::D1, DepthClass::MetadataDeep)
}

fn check_map1(manifest: &Manifest) -> CheckOutcome {
    for mapping in manifest.mappings() {
        let Some(artifact) = artifact_for_role(manifest, &mapping.artifact_role) else {
            return CheckOutcome::ran_fail(
                CheckId::MAP1,
                DepthClass::MetadataDeep,
                format!(
                    "mapping artifact_role {:?} does not resolve",
                    mapping.artifact_role.as_str()
                ),
            );
        };
        if !is_mapping_format(artifact.format) {
            return CheckOutcome::ran_fail(
                CheckId::MAP1,
                DepthClass::MetadataDeep,
                format!(
                    "mapping artifact_role {:?} resolves to non-mapping format {}",
                    mapping.artifact_role.as_str(),
                    artifact.format.as_str()
                ),
            );
        }
    }
    CheckOutcome::ran_pass(CheckId::MAP1, DepthClass::MetadataDeep)
}

fn check_f1(package_root: &Path, manifest: &Manifest) -> CheckOutcome {
    for artifact in manifest
        .artifacts()
        .iter()
        .filter(|artifact| artifact.format != ArtifactFormat::FieldRegistryV1)
    {
        if let Err(detail) = check_artifact_shape(package_root, artifact) {
            return CheckOutcome::ran_fail(CheckId::F1, DepthClass::MetadataDeep, detail);
        }
    }
    CheckOutcome::ran_pass(CheckId::F1, DepthClass::MetadataDeep)
}

fn check_artifact_shape(package_root: &Path, artifact: &Artifact) -> Result<(), String> {
    let path = package_root.join(artifact.path.as_str());
    match artifact.format {
        ArtifactFormat::Cog => read_cog_metadata(&path)
            .map(|_| ())
            .map_err(|err| format!("{}: {err}", artifact.path.as_str())),
        ArtifactFormat::Zarr => read_zarr_metadata(&path)
            .map(|_| ())
            .map_err(|err| format!("{}: {err}", artifact.path.as_str())),
        ArtifactFormat::GeoparquetReachTopologyV1 => {
            let meta = read_geoparquet_metadata(&path)
                .map_err(|err| format!("{}: {err}", artifact.path.as_str()))?;
            let columns: Vec<&str> = meta.schema().fields().iter().map(|f| f.name().as_str()).collect();
            require_columns(artifact, &columns, &["reach_id", "order_index", "manning_n", "width_m", "slope", "length_m", "geometry"])?;
            if !meta.has_geometry_column() {
                return Err(format!("{} format {} has no geometry column", artifact.path.as_str(), artifact.format.as_str()));
            }
            Ok(())
        }
        ArtifactFormat::ParquetGaugeLongV1 => check_parquet_columns(package_root, artifact, &["timestep", "gauge_id", "value"]),
        ArtifactFormat::ParquetGaugeMetadataV1 => check_parquet_columns(package_root, artifact, &["gauge_id", "x", "y", "z", "name"]),
        ArtifactFormat::ParquetCellToGaugeV1 => check_parquet_columns(package_root, artifact, &["cell_index", "gauge_id", "weight"]),
        ArtifactFormat::ParquetCellToReachV1 => check_parquet_columns(package_root, artifact, &["cell_index", "reach_id", "weight"]),
        ArtifactFormat::ParquetDomainAttributesV1 => check_parquet_columns(package_root, artifact, &["entity_index"]),
        ArtifactFormat::ParquetDomainMappingV1 => check_parquet_columns(package_root, artifact, &["source_index", "target_index", "weight"]),
        ArtifactFormat::FieldRegistryV1 => Ok(()),
    }
}

fn check_parquet_columns(
    package_root: &Path,
    artifact: &Artifact,
    required: &[&str],
) -> Result<(), String> {
    let path = package_root.join(artifact.path.as_str());
    let meta = read_parquet_metadata(&path)
        .map_err(|err| format!("{}: {err}", artifact.path.as_str()))?;
    require_columns(artifact, &meta.column_names(), required)
}

fn require_columns(artifact: &Artifact, columns: &[&str], required: &[&str]) -> Result<(), String> {
    let missing = required
        .iter()
        .copied()
        .filter(|name| !columns.contains(name))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} format {} missing required column(s): {}",
            artifact.path.as_str(),
            artifact.format.as_str(),
            missing.join(", ")
        ))
    }
}

fn load_registry_for_validation(
    package_root: &Path,
    manifest: &Manifest,
) -> Option<Result<FieldRegistry, String>> {
    let registry_artifacts = registry_artifacts(manifest);
    if registry_artifacts.len() != 1 {
        return Some(Err(format!(
            "expected exactly one registry.fields artifact with format {}, found {}",
            ArtifactFormat::FieldRegistryV1.as_str(),
            registry_artifacts.len()
        )));
    }
    let artifact = registry_artifacts[0];
    let path = package_root.join(artifact.path.as_str());
    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(err) => {
            return Some(Err(format!(
                "{}: registry unreadable: {err}",
                artifact.path.as_str()
            )));
        }
    };
    Some(FieldRegistry::from_json(&json).map_err(|err| err.to_string()))
}

fn registry_artifacts(manifest: &Manifest) -> Vec<&Artifact> {
    manifest
        .artifacts()
        .iter()
        .filter(|artifact| {
            artifact.role.as_str() == "registry.fields"
                && artifact.format == ArtifactFormat::FieldRegistryV1
        })
        .collect()
}

fn artifact_for_role<'a>(manifest: &'a Manifest, role: &ArtifactRole) -> Option<&'a Artifact> {
    manifest
        .artifacts()
        .iter()
        .find(|artifact| artifact.role == *role)
}

fn is_mapping_format(format: ArtifactFormat) -> bool {
    matches!(
        format,
        ArtifactFormat::ParquetDomainMappingV1
            | ArtifactFormat::ParquetCellToReachV1
            | ArtifactFormat::ParquetCellToGaugeV1
    )
}
