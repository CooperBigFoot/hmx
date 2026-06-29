//! Two-stage manifest boundary parse with the format-version hard cut first.

use std::path::Path;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{debug, instrument, warn};

use crate::CoreError;
use crate::dto::{ArtifactDto, DomainDto, GridDto, ManifestDto, MappingDto};
use crate::types::{
    Artifact, ArtifactFormat, ArtifactRole, ArtifactTimeMeaning, Crs, Domain, DomainId,
    FormatVersion, Grid, GridExtent, GridOrigin, IndexBase, Mapping, MappingPurpose, PackageKind,
    PackageName, Producer, ProducerVersion, RelativePath, Sha256, Variable,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    format_version: FormatVersion,
    name: PackageName,
    created_at: OffsetDateTime,
    producer: Producer,
    producer_version: ProducerVersion,
    package_kind: PackageKind,
    crs: Crs,
    grid: Grid,
    domains: Vec<Domain>,
    mappings: Vec<Mapping>,
    artifacts: Vec<Artifact>,
}

impl Manifest {
    /// Parses a raw `manifest.json` string into typed domain values.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when JSON structure is invalid, the version hard
    /// cut fails, or a manifest-local field value is not representable.
    #[instrument(skip(json))]
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        let dto: ManifestDto = serde_json::from_str(json).map_err(map_serde_error)?;

        let format_version: FormatVersion = dto.format_version.parse()?;
        let package_kind: PackageKind = dto.package_kind.parse()?;
        let name = PackageName::new(require_non_empty(dto.name, "name")?);
        let producer = Producer::new(require_non_empty(dto.producer, "producer")?);
        let producer_version =
            ProducerVersion::new(require_non_empty(dto.producer_version, "producer_version")?);
        let created_at =
            OffsetDateTime::parse(&dto.created_at, &Rfc3339).map_err(|_| {
                warn!(value = %dto.created_at, "rejecting non-RFC-3339 created_at");
                CoreError::InvalidTimestamp {
                    value: dto.created_at.clone(),
                }
            })?;
        let crs = Crs::new(require_non_empty(dto.crs, "crs")?);
        let grid = convert_grid(dto.grid)?;
        let domains = dto
            .domains
            .into_iter()
            .map(convert_domain)
            .collect::<Result<Vec<_>, _>>()?;
        let mappings = dto
            .mappings
            .into_iter()
            .map(convert_mapping)
            .collect::<Result<Vec<_>, _>>()?;
        let artifacts = dto
            .artifacts
            .into_iter()
            .map(convert_artifact)
            .collect::<Result<Vec<_>, _>>()?;

        debug!(name = %name.as_str(), "parsed manifest");
        Ok(Self {
            format_version,
            name,
            created_at,
            producer,
            producer_version,
            package_kind,
            crs,
            grid,
            domains,
            mappings,
            artifacts,
        })
    }

    pub fn format_version(&self) -> FormatVersion {
        self.format_version
    }

    pub fn name(&self) -> &PackageName {
        &self.name
    }

    pub fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    pub fn producer(&self) -> &Producer {
        &self.producer
    }

    pub fn producer_version(&self) -> &ProducerVersion {
        &self.producer_version
    }

    pub fn package_kind(&self) -> PackageKind {
        self.package_kind
    }

    pub fn crs(&self) -> &Crs {
        &self.crs
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn domains(&self) -> &[Domain] {
        &self.domains
    }

    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }

    pub fn artifacts(&self) -> &[Artifact] {
        &self.artifacts
    }
}

#[instrument]
pub fn read(package_root: &Path) -> Result<Manifest, CoreError> {
    let path = package_root.join("manifest.json");
    let json = std::fs::read_to_string(&path).map_err(|e| CoreError::ManifestUnreadable {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    Manifest::from_json(&json)
}

fn require_non_empty(value: String, field: &'static str) -> Result<String, CoreError> {
    if value.is_empty() {
        warn!(field, "rejecting empty required manifest field");
        return Err(CoreError::EmptyField { field });
    }
    Ok(value)
}

fn validate_sha256(s: &str) -> Result<(), CoreError> {
    if s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Ok(());
    }

    warn!(value = s, "rejecting invalid artifact sha256");
    Err(CoreError::InvalidSha256 {
        value: s.to_string(),
    })
}

fn validate_relative_path(s: &str) -> Result<(), CoreError> {
    let reason = if s.is_empty() {
        Some("empty")
    } else if s.starts_with('/') {
        Some("absolute")
    } else if s.split('/').any(|segment| segment == "..") {
        Some("parent traversal")
    } else {
        None
    };

    if let Some(reason) = reason {
        warn!(path = s, reason, "rejecting invalid artifact path");
        return Err(CoreError::InvalidArtifactPath {
            path: s.to_string(),
            reason,
        });
    }

    Ok(())
}

fn convert_grid(dto: GridDto) -> Result<Grid, CoreError> {
    Ok(Grid {
        crs: Crs::new(require_non_empty(dto.crs, "grid.crs")?),
        extent: GridExtent {
            xmin: dto.extent.xmin,
            ymin: dto.extent.ymin,
            xmax: dto.extent.xmax,
            ymax: dto.extent.ymax,
        },
        cell_size: dto.cell_size,
        nx: dto.nx,
        ny: dto.ny,
        origin: dto.origin.parse::<GridOrigin>()?,
    })
}

fn convert_domain(dto: DomainDto) -> Result<Domain, CoreError> {
    Ok(Domain {
        id: DomainId::new(require_non_empty(dto.id, "domain.id")?),
        entity_count: dto.entity_count,
        index_base: dto.index_base.parse::<IndexBase>()?,
        external_ids: dto.external_ids,
    })
}

fn convert_mapping(dto: MappingDto) -> Result<Mapping, CoreError> {
    let purpose = dto.purpose.parse::<MappingPurpose>()?;
    let variable = match dto.variable {
        Some(value) => Some(Variable::new(require_non_empty(value, "mapping.variable")?)),
        None => None,
    };

    if purpose == MappingPurpose::CellToGauge && variable.is_none() {
        warn!("rejecting cell_to_gauge mapping without variable");
        return Err(CoreError::MappingMissingVariable);
    }

    Ok(Mapping {
        purpose,
        source_domain: DomainId::new(require_non_empty(dto.source_domain, "mapping.source_domain")?),
        target_domain: DomainId::new(require_non_empty(dto.target_domain, "mapping.target_domain")?),
        variable,
        artifact_role: ArtifactRole::new(require_non_empty(
            dto.artifact_role,
            "mapping.artifact_role",
        )?),
    })
}

fn convert_artifact(dto: ArtifactDto) -> Result<Artifact, CoreError> {
    let role = ArtifactRole::new(require_non_empty(dto.role, "artifact.role")?);
    validate_relative_path(&dto.path)?;
    let path = RelativePath::new(dto.path);
    let format = dto.format.parse::<ArtifactFormat>()?;
    validate_sha256(&dto.sha256)?;
    let sha256 = Sha256::new(dto.sha256);
    let crs = dto
        .crs
        .map(|value| require_non_empty(value, "artifact.crs").map(Crs::new))
        .transpose()?;
    let domain = dto
        .domain
        .map(|value| require_non_empty(value, "artifact.domain").map(DomainId::new))
        .transpose()?;
    let variable = dto
        .variable
        .map(|value| require_non_empty(value, "artifact.variable").map(Variable::new))
        .transpose()?;
    let time_meaning = dto
        .time_meaning
        .map(|value| value.parse::<ArtifactTimeMeaning>())
        .transpose()?;

    Ok(Artifact {
        role,
        path,
        format,
        sha256,
        size_bytes: dto.size_bytes,
        crs,
        domain,
        variable,
        unit: dto.unit,
        time_meaning,
        interval_seconds: dto.interval_seconds,
        row_count: dto.row_count,
        first_step_index: dto.first_step_index,
        last_step_index: dto.last_step_index,
    })
}

fn map_serde_error(err: serde_json::Error) -> CoreError {
    let message = err.to_string();

    if let Some(field) = extract_backticked(&message, "missing field") {
        warn!(field = %field, "rejecting manifest with a missing required field");
        return CoreError::MissingManifestField { field };
    }
    if let Some(field) = extract_backticked(&message, "unknown field") {
        warn!(field = %field, "rejecting manifest with an unexpected field");
        return CoreError::ExtraManifestField { field };
    }

    warn!(error = %message, "rejecting unparsable manifest JSON");
    CoreError::InvalidManifestJson { detail: message }
}

fn extract_backticked(message: &str, prefix: &str) -> Option<String> {
    if !message.starts_with(prefix) {
        return None;
    }
    let after = message.find('`')? + 1;
    let len = message[after..].find('`')?;
    Some(message[after..after + len].to_string())
}

#[cfg(test)]
mod tests {
    use crate::CoreError;
    use crate::manifest::Manifest;
    use crate::types::{ArtifactFormat, FormatVersion, MappingPurpose};

    const VALID_MANIFEST: &str = r#"{
  "format_version": "0.1",
  "name": "synthetic-glacier-mini",
  "created_at": "2026-06-29T00:00:00Z",
  "producer": "hmx-core-a3-test",
  "producer_version": "0.1.3",
  "package_kind": "input",
  "crs": "EPSG:32645",
  "grid": {
    "crs": "EPSG:32645",
    "extent": { "xmin": 0.0, "ymin": 0.0, "xmax": 1000.0, "ymax": 1000.0 },
    "cell_size": 250.0,
    "nx": 4,
    "ny": 4,
    "origin": "upper_left"
  },
  "domains": [
    { "id": "cell", "entity_count": 16, "index_base": "dense_zero_based" },
    { "id": "glacier", "entity_count": 3, "index_base": "dense_zero_based", "external_ids": [1, 2, 2001] }
  ],
  "mappings": [
    { "purpose": "cell_to_glacier", "source_domain": "cell", "target_domain": "glacier", "artifact_role": "mapping.cell_to_glacier" }
  ],
  "artifacts": [
    { "role": "registry.fields", "path": "registry/fields.json", "format": "hmx/field_registry_v1", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "size_bytes": 512 }
  ]
}"#;

    #[test]
    fn valid_manifest_parses_to_typed_values() {
        let manifest = parse_valid();
        assert_eq!(manifest.format_version(), FormatVersion::V0_1);
        assert_eq!(manifest.name().as_str(), "synthetic-glacier-mini");
        assert_eq!(manifest.crs().as_str(), "EPSG:32645");
        assert_eq!(manifest.grid().nx, 4);
        assert_eq!(manifest.domains().len(), 2);
        assert_eq!(manifest.domains()[1].external_ids, Some(vec![1, 2, 2001]));
        assert_eq!(manifest.mappings()[0].purpose, MappingPurpose::CellToGlacier);
        assert_eq!(manifest.artifacts()[0].format, ArtifactFormat::FieldRegistryV1);
    }

    #[test]
    fn unknown_format_version_rejects() {
        match parse_err(replace_once(r#""format_version": "0.1""#, r#""format_version": "0.2""#)) {
            CoreError::UnknownFormatVersion { found } => assert_eq!(found, "0.2"),
            other => panic!("expected UnknownFormatVersion, got {other:?}"),
        }
    }

    #[test]
    fn unknown_format_version_wins_over_empty_crs() {
        let json = replace_once(r#""format_version": "0.1""#, r#""format_version": "9.9""#);
        let json = json.replace(r#""crs": "EPSG:32645""#, r#""crs": """#);
        match parse_err(json) {
            CoreError::UnknownFormatVersion { found } => assert_eq!(found, "9.9"),
            other => panic!("expected UnknownFormatVersion, got {other:?}"),
        }
    }

    #[test]
    fn missing_crs_rejects() {
        match parse_err(remove_top_level_crs()) {
            CoreError::MissingManifestField { field } => assert_eq!(field, "crs"),
            other => panic!("expected MissingManifestField, got {other:?}"),
        }
    }

    #[test]
    fn empty_crs_rejects() {
        match parse_err(replace_once(r#""crs": "EPSG:32645""#, r#""crs": """#)) {
            CoreError::EmptyField { field } => assert_eq!(field, "crs"),
            other => panic!("expected EmptyField, got {other:?}"),
        }
    }

    #[test]
    fn extra_top_level_key_rejects() {
        match parse_err(VALID_MANIFEST.replace(
            r#""artifacts": ["#,
            r#""glacier_count": 3, "artifacts": ["#,
        )) {
            CoreError::ExtraManifestField { field } => assert_eq!(field, "glacier_count"),
            other => panic!("expected ExtraManifestField, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_field_rejects() {
        match parse_err(VALID_MANIFEST.replace(
            r#",
  "domains": [
    { "id": "cell", "entity_count": 16, "index_base": "dense_zero_based" },
    { "id": "glacier", "entity_count": 3, "index_base": "dense_zero_based", "external_ids": [1, 2, 2001] }
  ]"#,
            "",
        )) {
            CoreError::MissingManifestField { field } => assert_eq!(field, "domains"),
            other => panic!("expected MissingManifestField, got {other:?}"),
        }
    }

    #[test]
    fn invalid_created_at_rejects() {
        match parse_err(replace_once(
            r#""created_at": "2026-06-29T00:00:00Z""#,
            r#""created_at": "2026-06-01""#,
        )) {
            CoreError::InvalidTimestamp { value } => assert_eq!(value, "2026-06-01"),
            other => panic!("expected InvalidTimestamp, got {other:?}"),
        }
    }

    #[test]
    fn invalid_package_kind_rejects() {
        match parse_err(replace_once(r#""package_kind": "input""#, r#""package_kind": "output""#)) {
            CoreError::InvalidEnumValue { field, found } => {
                assert_eq!(field, "package_kind");
                assert_eq!(found, "output");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn invalid_sha256_rejects() {
        match parse_err(replace_once(
            r#""sha256": "0000000000000000000000000000000000000000000000000000000000000000""#,
            r#""sha256": "ABC""#,
        )) {
            CoreError::InvalidSha256 { value } => assert_eq!(value, "ABC"),
            other => panic!("expected InvalidSha256, got {other:?}"),
        }
    }

    #[test]
    fn invalid_artifact_paths_reject() {
        match parse_err(replace_once(
            r#""path": "registry/fields.json""#,
            r#""path": "/abs/x.tif""#,
        )) {
            CoreError::InvalidArtifactPath { reason, .. } => assert_eq!(reason, "absolute"),
            other => panic!("expected InvalidArtifactPath, got {other:?}"),
        }

        match parse_err(replace_once(
            r#""path": "registry/fields.json""#,
            r#""path": "../escape.tif""#,
        )) {
            CoreError::InvalidArtifactPath { reason, .. } => {
                assert_eq!(reason, "parent traversal");
            }
            other => panic!("expected InvalidArtifactPath, got {other:?}"),
        }
    }

    #[test]
    fn invalid_artifact_format_rejects() {
        match parse_err(replace_once(r#""format": "hmx/field_registry_v1""#, r#""format": "geotiff""#)) {
            CoreError::InvalidEnumValue { field, found } => {
                assert_eq!(field, "format");
                assert_eq!(found, "geotiff");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn invalid_mapping_purpose_rejects() {
        match parse_err(replace_once(
            r#""purpose": "cell_to_glacier""#,
            r#""purpose": "cell_to_lake""#,
        )) {
            CoreError::InvalidEnumValue { field, found } => {
                assert_eq!(field, "purpose");
                assert_eq!(found, "cell_to_lake");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn cell_to_gauge_requires_variable() {
        match parse_err(replace_once(
            r#""purpose": "cell_to_glacier""#,
            r#""purpose": "cell_to_gauge""#,
        )) {
            CoreError::MappingMissingVariable => {}
            other => panic!("expected MappingMissingVariable, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_rejects_without_panic() {
        assert!(Manifest::from_json("{ not json }").is_err());
    }

    fn parse_valid() -> Manifest {
        Manifest::from_json(VALID_MANIFEST).unwrap_or_else(|err| {
            panic!("expected valid manifest to parse, got {err:?}");
        })
    }

    fn parse_err(json: String) -> CoreError {
        Manifest::from_json(&json).unwrap_err()
    }

    fn replace_once(from: &str, to: &str) -> String {
        VALID_MANIFEST.replacen(from, to, 1)
    }

    fn remove_top_level_crs() -> String {
        VALID_MANIFEST.replacen(
            r#",
  "crs": "EPSG:32645""#,
            "",
            1,
        )
    }
}
