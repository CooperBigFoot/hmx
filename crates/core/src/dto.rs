use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestDto {
    pub(crate) format_version: String,
    pub(crate) name: String,
    pub(crate) created_at: String,
    pub(crate) producer: String,
    pub(crate) producer_version: String,
    pub(crate) package_kind: String,
    pub(crate) crs: String,
    pub(crate) grid: GridDto,
    pub(crate) domains: Vec<DomainDto>,
    pub(crate) mappings: Vec<MappingDto>,
    pub(crate) artifacts: Vec<ArtifactDto>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GridDto {
    pub(crate) crs: String,
    pub(crate) extent: ExtentDto,
    pub(crate) cell_size: f64,
    pub(crate) nx: u32,
    pub(crate) ny: u32,
    pub(crate) origin: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtentDto {
    pub(crate) xmin: f64,
    pub(crate) ymin: f64,
    pub(crate) xmax: f64,
    pub(crate) ymax: f64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DomainDto {
    pub(crate) id: String,
    pub(crate) entity_count: u64,
    pub(crate) index_base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) external_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingDto {
    pub(crate) purpose: String,
    pub(crate) source_domain: String,
    pub(crate) target_domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) variable: Option<String>,
    pub(crate) artifact_role: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactDto {
    pub(crate) role: String,
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) crs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) variable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) time_meaning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interval_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) row_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first_step_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_step_index: Option<u64>,
}
