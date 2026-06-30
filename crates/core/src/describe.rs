//! Frontend-agnostic `describe` engine.
//!
//! `describe` emits facts only, never a conformance verdict (spec §10.2). It
//! reads and parses the manifest first, computes the A7 content-hash from that
//! manifest, then reads only the optional field registry to surface field facts.

use std::path::Path;

use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use tracing::{info, instrument};

use crate::hash::ContentHash;
use crate::manifest::Manifest;
use crate::registry::{FieldRegistry, FieldSpec};
use crate::report::DescribeError;
use crate::types::{Artifact, ArtifactFormat, Domain, Grid, Mapping};

#[derive(Debug, Clone, PartialEq)]
pub struct Description {
    manifest: Manifest,
    content_hash: ContentHash,
    fields: Vec<FieldSpec>,
}

impl Description {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn fields(&self) -> &[FieldSpec] {
        &self.fields
    }

    pub fn to_dto(&self) -> DescriptionDto<'_> {
        DescriptionDto::from(self)
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.to_dto())
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.to_dto())
    }
}

#[instrument(fields(package_root = %package_root.as_ref().display()))]
pub fn describe(package_root: impl AsRef<Path>) -> Result<Description, DescribeError> {
    let package_root = package_root.as_ref();
    let manifest_path = package_root.join("manifest.json");
    let json =
        std::fs::read_to_string(&manifest_path).map_err(|e| DescribeError::ManifestUnreadable {
            path: manifest_path.display().to_string(),
            detail: e.to_string(),
        })?;
    let manifest = Manifest::from_json(&json).map_err(DescribeError::Manifest)?;
    let content_hash = manifest.content_hash().map_err(DescribeError::Manifest)?;
    let fields = read_registry_fields(package_root, &manifest)?;
    info!("assembled description");
    Ok(Description {
        manifest,
        content_hash,
        fields,
    })
}

#[instrument(fields(package_root = %package_root.as_ref().display()))]
pub fn describe_json(package_root: impl AsRef<Path>) -> Result<String, DescribeError> {
    describe(package_root)?
        .to_json_string()
        .map_err(|e| DescribeError::Serialize {
            detail: e.to_string(),
        })
}

fn read_registry_fields(
    package_root: &Path,
    manifest: &Manifest,
) -> Result<Vec<FieldSpec>, DescribeError> {
    let Some(artifact) = manifest.artifacts().iter().find(|artifact| {
        artifact.role.as_str() == "registry.fields"
            && artifact.format == ArtifactFormat::FieldRegistryV1
    }) else {
        return Ok(Vec::new());
    };
    let path = package_root.join(artifact.path.as_str());
    let json = std::fs::read_to_string(&path).map_err(|e| DescribeError::Registry {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    let registry = FieldRegistry::from_json(&json).map_err(|e| DescribeError::Registry {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    Ok(registry.iter().cloned().collect())
}

#[derive(Serialize)]
pub struct DescriptionDto<'a> {
    manifest: ManifestFloorDto<'a>,
    content_hash: ContentHashDto<'a>,
    crs: &'a str,
    grid: GridDto<'a>,
    domains: Vec<DomainFactDto<'a>>,
    fields: Vec<FieldFactDto<'a>>,
    mappings: Vec<MappingFactDto<'a>>,
    artifacts: Vec<ArtifactFactDto<'a>>,
}

impl<'a> From<&'a Description> for DescriptionDto<'a> {
    fn from(description: &'a Description) -> Self {
        let manifest = &description.manifest;
        Self {
            manifest: ManifestFloorDto {
                format_version: manifest.format_version().as_str(),
                name: manifest.name().as_str(),
                created_at: match manifest.created_at().format(&Rfc3339) {
                    Ok(value) => value,
                    Err(_) => manifest.created_at().to_string(),
                },
                producer: manifest.producer().as_str(),
                producer_version: manifest.producer_version().as_str(),
                package_kind: manifest.package_kind().as_str(),
                crs: manifest.crs().as_str(),
            },
            content_hash: ContentHashDto {
                algo: description.content_hash.hash_algo(),
                value: description.content_hash.as_str(),
            },
            crs: manifest.crs().as_str(),
            grid: GridDto::from(manifest.grid()),
            domains: manifest.domains().iter().map(DomainFactDto::from).collect(),
            fields: description.fields.iter().map(FieldFactDto::from).collect(),
            mappings: manifest.mappings().iter().map(MappingFactDto::from).collect(),
            artifacts: manifest.artifacts().iter().map(ArtifactFactDto::from).collect(),
        }
    }
}

#[derive(Serialize)]
struct ManifestFloorDto<'a> {
    format_version: &'a str,
    name: &'a str,
    created_at: String,
    producer: &'a str,
    producer_version: &'a str,
    package_kind: &'a str,
    crs: &'a str,
}

#[derive(Serialize)]
struct ContentHashDto<'a> {
    algo: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct GridDto<'a> {
    crs: &'a str,
    extent: GridExtentDto,
    cell_size: f64,
    nx: u32,
    ny: u32,
    origin: &'a str,
}

impl<'a> From<&'a Grid> for GridDto<'a> {
    fn from(grid: &'a Grid) -> Self {
        Self {
            crs: grid.crs.as_str(),
            extent: GridExtentDto {
                xmin: grid.extent.xmin,
                ymin: grid.extent.ymin,
                xmax: grid.extent.xmax,
                ymax: grid.extent.ymax,
            },
            cell_size: grid.cell_size,
            nx: grid.nx,
            ny: grid.ny,
            origin: grid.origin.as_str(),
        }
    }
}

#[derive(Serialize)]
struct GridExtentDto {
    xmin: f64,
    ymin: f64,
    xmax: f64,
    ymax: f64,
}

#[derive(Serialize)]
struct DomainFactDto<'a> {
    id: &'a str,
    entity_count: u64,
    index_base: &'a str,
    external_id_count: Option<usize>,
}

impl<'a> From<&'a Domain> for DomainFactDto<'a> {
    fn from(domain: &'a Domain) -> Self {
        Self {
            id: domain.id.as_str(),
            entity_count: domain.entity_count,
            index_base: domain.index_base.as_str(),
            external_id_count: domain.external_ids.as_ref().map(Vec::len),
        }
    }
}

#[derive(Serialize)]
struct FieldFactDto<'a> {
    id: &'a str,
    domain: &'a str,
    quantity: &'a str,
    units: &'a str,
    value_type: &'a str,
    time_meaning: &'a str,
    role: &'a str,
    conservation_class: &'a str,
    extent: &'a str,
}

impl<'a> From<&'a FieldSpec> for FieldFactDto<'a> {
    fn from(field: &'a FieldSpec) -> Self {
        Self {
            id: field.id().as_str(),
            domain: field.domain().as_str(),
            quantity: field.quantity().as_str(),
            units: field.units().as_str(),
            value_type: field.value_type().as_str(),
            time_meaning: field.time_meaning().as_str(),
            role: field.role().as_str(),
            conservation_class: field.conservation_class().as_str(),
            extent: field.extent().as_str(),
        }
    }
}

#[derive(Serialize)]
struct MappingFactDto<'a> {
    purpose: &'a str,
    source_domain: &'a str,
    target_domain: &'a str,
    variable: Option<&'a str>,
    artifact_role: &'a str,
}

impl<'a> From<&'a Mapping> for MappingFactDto<'a> {
    fn from(mapping: &'a Mapping) -> Self {
        Self {
            purpose: mapping.purpose.as_str(),
            source_domain: mapping.source_domain.as_str(),
            target_domain: mapping.target_domain.as_str(),
            variable: mapping.variable.as_ref().map(|variable| variable.as_str()),
            artifact_role: mapping.artifact_role.as_str(),
        }
    }
}

#[derive(Serialize)]
struct ArtifactFactDto<'a> {
    role: &'a str,
    path: &'a str,
    format: &'a str,
    sha256: &'a str,
    size_bytes: Option<u64>,
}

impl<'a> From<&'a Artifact> for ArtifactFactDto<'a> {
    fn from(artifact: &'a Artifact) -> Self {
        Self {
            role: artifact.role.as_str(),
            path: artifact.path.as_str(),
            format: artifact.format.as_str(),
            sha256: artifact.sha256.as_str(),
            size_bytes: artifact.size_bytes,
        }
    }
}
