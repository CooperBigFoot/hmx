//! Canonical manifest JSON bytes for the package content-hash.

use serde_json::{Map, Value};
use time::format_description::well_known::Rfc3339;

use crate::CoreError;
use crate::dto::{ArtifactDto, DomainDto, ExtentDto, GridDto, ManifestDto, MappingDto};
use crate::manifest::Manifest;
use crate::types::{Artifact, Domain, Grid, GridExtent, Mapping};

pub(crate) fn canonical_bytes(manifest: &Manifest) -> Result<Vec<u8>, CoreError> {
    let dto = ManifestDto::try_from(manifest)?;
    let value = serde_json::to_value(&dto).map_err(|e| CoreError::CanonicalizeFailed {
        detail: e.to_string(),
    })?;
    let canonical = serde_json::to_string(&sort_json_value(value)).map_err(|e| {
        CoreError::CanonicalizeFailed {
            detail: e.to_string(),
        }
    })?;
    Ok(canonical.into_bytes())
}

fn sort_json_value(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));

            let mut sorted = Map::new();
            for (key, value) in entries {
                sorted.insert(key, sort_json_value(value));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json_value).collect()),
        other => other,
    }
}

impl TryFrom<&Manifest> for ManifestDto {
    type Error = CoreError;

    fn try_from(manifest: &Manifest) -> Result<Self, Self::Error> {
        let created_at =
            manifest
                .created_at()
                .format(&Rfc3339)
                .map_err(|e| CoreError::CanonicalizeFailed {
                    detail: e.to_string(),
                })?;

        Ok(Self {
            format_version: manifest.format_version().as_str().to_string(),
            name: manifest.name().as_str().to_string(),
            created_at,
            producer: manifest.producer().as_str().to_string(),
            producer_version: manifest.producer_version().as_str().to_string(),
            package_kind: manifest.package_kind().as_str().to_string(),
            crs: manifest.crs().as_str().to_string(),
            grid: GridDto::from(manifest.grid()),
            domains: manifest.domains().iter().map(DomainDto::from).collect(),
            mappings: manifest.mappings().iter().map(MappingDto::from).collect(),
            artifacts: manifest.artifacts().iter().map(ArtifactDto::from).collect(),
        })
    }
}

impl From<&Grid> for GridDto {
    fn from(grid: &Grid) -> Self {
        Self {
            crs: grid.crs.as_str().to_string(),
            extent: ExtentDto::from(&grid.extent),
            cell_size: grid.cell_size,
            nx: grid.nx,
            ny: grid.ny,
            origin: grid.origin.as_str().to_string(),
        }
    }
}

impl From<&GridExtent> for ExtentDto {
    fn from(extent: &GridExtent) -> Self {
        Self {
            xmin: extent.xmin,
            ymin: extent.ymin,
            xmax: extent.xmax,
            ymax: extent.ymax,
        }
    }
}

impl From<&Domain> for DomainDto {
    fn from(domain: &Domain) -> Self {
        Self {
            id: domain.id.as_str().to_string(),
            entity_count: domain.entity_count,
            index_base: domain.index_base.as_str().to_string(),
            external_ids: domain.external_ids.clone(),
        }
    }
}

impl From<&Mapping> for MappingDto {
    fn from(mapping: &Mapping) -> Self {
        Self {
            purpose: mapping.purpose.as_str().to_string(),
            source_domain: mapping.source_domain.as_str().to_string(),
            target_domain: mapping.target_domain.as_str().to_string(),
            variable: mapping.variable.as_ref().map(|value| value.as_str().to_string()),
            artifact_role: mapping.artifact_role.as_str().to_string(),
        }
    }
}

impl From<&Artifact> for ArtifactDto {
    fn from(artifact: &Artifact) -> Self {
        Self {
            role: artifact.role.as_str().to_string(),
            path: artifact.path.as_str().to_string(),
            format: artifact.format.as_str().to_string(),
            sha256: artifact.sha256.as_str().to_string(),
            size_bytes: artifact.size_bytes,
            crs: artifact.crs.as_ref().map(|value| value.as_str().to_string()),
            domain: artifact.domain.as_ref().map(|value| value.as_str().to_string()),
            variable: artifact.variable.as_ref().map(|value| value.as_str().to_string()),
            unit: artifact.unit.clone(),
            time_meaning: artifact
                .time_meaning
                .as_ref()
                .map(|value| value.as_str().to_string()),
            interval_seconds: artifact.interval_seconds,
            row_count: artifact.row_count,
            first_step_index: artifact.first_step_index,
            last_step_index: artifact.last_step_index,
        }
    }
}
