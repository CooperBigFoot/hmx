//! Inert typed values for the HMX manifest.

use std::fmt;
use std::str::FromStr;

use crate::CoreError;

macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_newtype!(PackageName);
string_newtype!(Producer);
string_newtype!(ProducerVersion);
string_newtype!(Crs);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainId(String);

impl DomainId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A field's identity in the registry (spec §6.2). Keys the `FieldRegistry`
/// map, so it is hashable and orderable for deterministic iteration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldId(String);

impl FieldId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

string_newtype!(ArtifactRole);
string_newtype!(Variable);
string_newtype!(Sha256);
string_newtype!(RelativePath);
string_newtype!(Quantity);
string_newtype!(Units);

macro_rules! closed_enum {
    ($name:ident, $field:literal, [$(($variant:ident, $value:literal)),+ $(,)?]) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($value => Ok(Self::$variant),)+
                    other => Err(CoreError::InvalidEnumValue {
                        field: $field,
                        found: other.to_string(),
                    }),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatVersion {
    V0_1,
}

impl FormatVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V0_1 => "0.1",
        }
    }
}

impl FromStr for FormatVersion {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "0.1" => Ok(Self::V0_1),
            other => Err(CoreError::UnknownFormatVersion {
                found: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

closed_enum!(PackageKind, "package_kind", [(Input, "input")]);
closed_enum!(IndexBase, "index_base", [(DenseZeroBased, "dense_zero_based")]);
closed_enum!(GridOrigin, "origin", [(UpperLeft, "upper_left")]);
closed_enum!(
    MappingPurpose,
    "purpose",
    [
        (CellToReach, "cell_to_reach"),
        (CellToGlacier, "cell_to_glacier"),
        (GlacierToCell, "glacier_to_cell"),
        (CellToGauge, "cell_to_gauge"),
    ]
);
closed_enum!(
    ArtifactFormat,
    "format",
    [
        (Cog, "cog"),
        (Zarr, "zarr"),
        (GeoparquetReachTopologyV1, "geoparquet/reach_topology_v1"),
        (ParquetGaugeLongV1, "parquet/gauge_long_v1"),
        (ParquetGaugeMetadataV1, "parquet/gauge_metadata_v1"),
        (ParquetCellToGaugeV1, "parquet/cell_to_gauge_v1"),
        (ParquetCellToReachV1, "parquet/cell_to_reach_v1"),
        (ParquetDomainAttributesV1, "parquet/domain_attributes_v1"),
        (ParquetDomainMappingV1, "parquet/domain_mapping_v1"),
        (FieldRegistryV1, "hmx/field_registry_v1"),
    ]
);
closed_enum!(
    ArtifactTimeMeaning,
    "time_meaning",
    [
        (Instant, "instant"),
        (Rate, "rate"),
        (StepAmount, "step_amount"),
        (MeanOverInterval, "mean_over_interval"),
        (
            AccumulatedOverInterval,
            "accumulated_over_interval"
        ),
    ]
);
closed_enum!(
    ValueType,
    "value_type",
    [
        (F32, "f32"),
        (F64, "f64"),
        (I32, "i32"),
        (I64, "i64"),
        (Bool, "bool"),
    ]
);
closed_enum!(
    FieldTimeMeaning,
    "time_meaning",
    [
        (Instant, "instant"),
        (Rate, "rate"),
        (StepAmount, "step_amount"),
    ]
);
closed_enum!(
    SemanticRole,
    "role",
    [
        (DifferentialState, "differential_state"),
        (Parameter, "parameter"),
        (Forcing, "forcing"),
        (Coupling, "coupling"),
        (Diagnostic, "diagnostic"),
    ]
);
closed_enum!(
    ConservationClass,
    "conservation_class",
    [
        (WaterVolume, "water_volume"),
        (Energy, "energy"),
        (None, "none"),
    ]
);
closed_enum!(Extent, "extent", [(Scalar, "scalar"), (PerLayer, "per_layer")]);

/// The raster grid the gridded artifacts live on (spec §4.2). Plain data; A3
/// performs no numeric-range checks (cell_size>0, nx>=1) — those are the A8
/// validator's, not the reader's.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub crs: Crs,
    pub extent: GridExtent,
    pub cell_size: f64,
    pub nx: u32,
    pub ny: u32,
    pub origin: GridOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridExtent {
    pub xmin: f64,
    pub ymin: f64,
    pub xmax: f64,
    pub ymax: f64,
}

/// A multi-entity domain (spec §5). `entity_count` is the SOLE authoritative
/// cardinality (OD4). A3 parses `external_ids` verbatim; the §5.4 length ==
/// entity_count and dense-index cross-check is A6, not A3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Domain {
    pub id: DomainId,
    pub entity_count: u64,
    pub index_base: IndexBase,
    pub external_ids: Option<Vec<i64>>,
}

/// An explicit cross-domain mapping declaration (spec §8). A3 parses each
/// element independently; it does NOT resolve `source_domain`/`target_domain`
/// against declared domains or `artifact_role` against declared artifacts (A6/A8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub purpose: MappingPurpose,
    pub source_domain: DomainId,
    pub target_domain: DomainId,
    pub variable: Option<Variable>,
    pub artifact_role: ArtifactRole,
}

/// One declared on-disk artifact (spec §7.2). A3 records the declared metadata;
/// it never opens the artifact bytes (that is A4/A4b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub role: ArtifactRole,
    pub path: RelativePath,
    pub format: ArtifactFormat,
    pub sha256: Sha256,
    pub size_bytes: Option<u64>,
    pub crs: Option<Crs>,
    pub domain: Option<DomainId>,
    pub variable: Option<Variable>,
    pub unit: Option<String>,
    pub time_meaning: Option<ArtifactTimeMeaning>,
    pub interval_seconds: Option<u64>,
    pub row_count: Option<u64>,
    pub first_step_index: Option<u64>,
    pub last_step_index: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::CoreError;
    use crate::types::{
        ArtifactFormat, ArtifactTimeMeaning, ConservationClass, Crs, Extent,
        FieldTimeMeaning, FormatVersion, GridOrigin, IndexBase, MappingPurpose, PackageKind,
        SemanticRole, ValueType,
    };

    #[test]
    fn enum_values_round_trip() {
        assert_round_trip::<FormatVersion>(&["0.1"]);
        assert_round_trip::<PackageKind>(&["input"]);
        assert_round_trip::<IndexBase>(&["dense_zero_based"]);
        assert_round_trip::<GridOrigin>(&["upper_left"]);
        assert_round_trip::<MappingPurpose>(&[
            "cell_to_reach",
            "cell_to_glacier",
            "glacier_to_cell",
            "cell_to_gauge",
        ]);
        assert_round_trip::<ArtifactFormat>(&[
            "cog",
            "zarr",
            "geoparquet/reach_topology_v1",
            "parquet/gauge_long_v1",
            "parquet/gauge_metadata_v1",
            "parquet/cell_to_gauge_v1",
            "parquet/cell_to_reach_v1",
            "parquet/domain_attributes_v1",
            "parquet/domain_mapping_v1",
            "hmx/field_registry_v1",
        ]);
        assert_round_trip::<ArtifactTimeMeaning>(&[
            "instant",
            "rate",
            "step_amount",
            "mean_over_interval",
            "accumulated_over_interval",
        ]);
        assert_round_trip::<ValueType>(&["f32", "f64", "i32", "i64", "bool"]);
        assert_round_trip::<FieldTimeMeaning>(&["instant", "rate", "step_amount"]);
        assert_round_trip::<SemanticRole>(&[
            "differential_state",
            "parameter",
            "forcing",
            "coupling",
            "diagnostic",
        ]);
        assert_round_trip::<ConservationClass>(&["water_volume", "energy", "none"]);
        assert_round_trip::<Extent>(&["scalar", "per_layer"]);
    }

    #[test]
    fn unknown_format_version_is_a_hard_cut() {
        match FormatVersion::from_str("0.2") {
            Err(CoreError::UnknownFormatVersion { found }) => assert_eq!(found, "0.2"),
            other => panic!("expected UnknownFormatVersion, got {other:?}"),
        }
    }

    #[test]
    fn unknown_artifact_format_reports_field_name() {
        match ArtifactFormat::from_str("geotiff") {
            Err(CoreError::InvalidEnumValue { field, found }) => {
                assert_eq!(field, "format");
                assert_eq!(found, "geotiff");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn newtypes_round_trip() {
        assert_eq!(Crs::new("EPSG:32645").as_str(), "EPSG:32645");
    }

    fn assert_round_trip<T>(values: &[&str])
    where
        T: FromStr + ToString + std::fmt::Debug + PartialEq,
        <T as FromStr>::Err: std::fmt::Debug,
    {
        for value in values {
            let parsed = T::from_str(value).unwrap_or_else(|err| {
                panic!("expected {value:?} to parse, got {err:?}");
            });
            assert_eq!(parsed.to_string(), *value);
        }
    }
}
