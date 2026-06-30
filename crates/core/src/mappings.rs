//! Typed cross-domain mapping geometry (spec §8, divergence D3).

use std::path::Path;

use tracing::{debug, instrument};

use crate::CoreError;
use crate::domains::MappingSide;
use crate::readers::control_plane::{DomainMapping, read_domain_mapping};
use crate::types::{ArtifactRole, DomainId, Mapping, MappingPurpose, Variable};

/// Represent one on-disk `domain_mapping_v1` row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MappingEntry {
    pub source_index: i64,
    pub target_index: i64,
    pub weight: f64,
}

/// Join a manifest mapping declaration with its materialized on-disk rows.
#[derive(Debug, Clone, PartialEq)]
pub struct MappingGeometry {
    purpose: MappingPurpose,
    source_domain: DomainId,
    target_domain: DomainId,
    variable: Option<Variable>,
    artifact_role: ArtifactRole,
    entries: Vec<MappingEntry>,
}

impl MappingGeometry {
    /// Build a mapping geometry from an A3 declaration and A4b rows.
    #[instrument(skip(declaration, rows))]
    pub fn from_declaration(declaration: &Mapping, rows: &DomainMapping) -> Self {
        let entries = rows
            .source_index()
            .iter()
            .zip(rows.target_index())
            .zip(rows.weight())
            .map(|((&source_index, &target_index), &weight)| MappingEntry {
                source_index,
                target_index,
                weight,
            })
            .collect();

        Self {
            purpose: declaration.purpose,
            source_domain: declaration.source_domain.clone(),
            target_domain: declaration.target_domain.clone(),
            variable: declaration.variable.clone(),
            artifact_role: declaration.artifact_role.clone(),
            entries,
        }
    }

    /// Read A4b mapping rows and join them to a manifest declaration.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the underlying `domain_mapping_v1` table cannot
    /// be materialized by A4b.
    #[instrument(skip(declaration), fields(path = %path.as_ref().display()))]
    pub fn read(path: impl AsRef<Path>, declaration: &Mapping) -> Result<Self, CoreError> {
        let rows = read_domain_mapping(path)?;
        let geometry = Self::from_declaration(declaration, &rows);
        debug!(
            entries = geometry.num_entries(),
            purpose = %geometry.purpose,
            "built mapping geometry"
        );
        Ok(geometry)
    }

    pub fn purpose(&self) -> MappingPurpose {
        self.purpose
    }

    pub fn source_domain(&self) -> &DomainId {
        &self.source_domain
    }

    pub fn target_domain(&self) -> &DomainId {
        &self.target_domain
    }

    pub fn variable(&self) -> Option<&Variable> {
        self.variable.as_ref()
    }

    pub fn artifact_role(&self) -> &ArtifactRole {
        &self.artifact_role
    }

    pub fn entries(&self) -> &[MappingEntry] {
        &self.entries
    }

    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }

    /// Resolve which side of this mapping references `domain`.
    pub fn side_for_domain(&self, domain: &DomainId) -> Option<MappingSide> {
        if domain == &self.target_domain {
            Some(MappingSide::Target)
        } else if domain == &self.source_domain {
            Some(MappingSide::Source)
        } else {
            None
        }
    }

    /// Iterate entity indices on the requested side of this mapping.
    pub fn indices_on(&self, side: MappingSide) -> impl Iterator<Item = i64> + '_ {
        self.entries.iter().map(move |entry| match side {
            MappingSide::Source => entry.source_index,
            MappingSide::Target => entry.target_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use arrow::array::{Float64Array, Int64Array};
    use arrow::datatypes::{DataType, Field as ArrowField, Schema};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::ArrowWriter;

    use crate::domains::MappingSide;
    use crate::mappings::{MappingEntry, MappingGeometry};
    use crate::readers::control_plane::read_domain_mapping;
    use crate::types::{ArtifactRole, DomainId, Mapping, MappingPurpose, Variable};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hmx-{tag}-{}-{}.parquet",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn mapping_declaration() -> Mapping {
        Mapping {
            purpose: MappingPurpose::CellToGauge,
            source_domain: DomainId::new("cell"),
            target_domain: DomainId::new("gauge"),
            variable: Some(Variable::new("precipitation")),
            artifact_role: ArtifactRole::new("mapping.cell_to_gauge.precipitation"),
        }
    }

    fn write_mapping(path: &PathBuf) {
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("source_index", DataType::Int64, false),
            ArrowField::new("target_index", DataType::Int64, false),
            ArrowField::new("weight", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(vec![10, 11])),
                Arc::new(Int64Array::from(vec![0, 1])),
                Arc::new(Float64Array::from(vec![0.25, 0.75])),
            ],
        )
        .expect("synthetic record batch must build");
        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, schema, None)
                .expect("parquet writer must construct");
            writer.write(&batch).expect("batch write must succeed");
            writer.close().expect("parquet writer close must succeed");
        }
        std::fs::write(path, buffer).expect("temp parquet write must succeed");
    }

    #[test]
    fn from_declaration_zips_rows() {
        let path = temp_path("mapping-zip");
        write_mapping(&path);
        let rows = read_domain_mapping(&path).expect("mapping must read");
        std::fs::remove_file(&path).ok();

        let declaration = mapping_declaration();
        let geometry = MappingGeometry::from_declaration(&declaration, &rows);

        assert_eq!(geometry.purpose(), MappingPurpose::CellToGauge);
        assert_eq!(geometry.source_domain().as_str(), "cell");
        assert_eq!(geometry.target_domain().as_str(), "gauge");
        assert_eq!(
            geometry.variable().map(Variable::as_str),
            Some("precipitation")
        );
        assert_eq!(
            geometry.artifact_role().as_str(),
            "mapping.cell_to_gauge.precipitation"
        );
        assert_eq!(
            geometry.entries(),
            &[
                MappingEntry {
                    source_index: 10,
                    target_index: 0,
                    weight: 0.25,
                },
                MappingEntry {
                    source_index: 11,
                    target_index: 1,
                    weight: 0.75,
                },
            ]
        );
    }

    #[test]
    fn side_for_domain_resolves_both_sides() {
        let path = temp_path("mapping-side");
        write_mapping(&path);
        let rows = read_domain_mapping(&path).expect("mapping must read");
        std::fs::remove_file(&path).ok();

        let geometry = MappingGeometry::from_declaration(&mapping_declaration(), &rows);

        assert_eq!(
            geometry.side_for_domain(&DomainId::new("cell")),
            Some(MappingSide::Source)
        );
        assert_eq!(
            geometry.side_for_domain(&DomainId::new("gauge")),
            Some(MappingSide::Target)
        );
        assert_eq!(geometry.side_for_domain(&DomainId::new("lake")), None);
    }
}
