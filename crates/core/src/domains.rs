//! Multi-entity domains, OD4 cross-checks, and additive same-domain projection.

use std::collections::BTreeSet;

use tracing::{debug, instrument};

use crate::mappings::MappingGeometry;
use crate::readers::control_plane::DomainAttributes;
use crate::types::{Domain, DomainId, IndexBase};

/// Identify which side of a `MappingGeometry` references a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingSide {
    Source,
    Target,
}

/// Represent a typed view of a manifest multi-entity domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainDescriptor {
    domain: DomainId,
    entity_count: u64,
    index_base: IndexBase,
    external_ids: Option<Vec<i64>>,
}

/// Hold one additive field projected over a domain's dense index.
#[derive(Debug, Clone, PartialEq)]
pub struct AdditiveField {
    domain: DomainId,
    field_id: String,
    values: Vec<f64>,
}

impl DomainDescriptor {
    /// Build a descriptor from the manifest domain authority.
    ///
    /// # Errors
    ///
    /// Returns [`CardinalityError::ExternalIdsLengthMismatch`] when
    /// `external_ids` is present but not the same length as `entity_count`.
    #[instrument(skip(domain))]
    pub fn from_domain(domain: &Domain) -> Result<Self, CardinalityError> {
        let entity_count = domain.entity_count;
        if let Some(ids) = &domain.external_ids
            && ids.len() as u64 != entity_count
        {
            return Err(CardinalityError::ExternalIdsLengthMismatch {
                domain: domain.id.as_str().to_string(),
                entity_count,
                external_ids_len: ids.len(),
            });
        }

        Ok(Self {
            domain: domain.id.clone(),
            entity_count,
            index_base: domain.index_base,
            external_ids: domain.external_ids.clone(),
        })
    }

    pub fn domain(&self) -> &DomainId {
        &self.domain
    }

    pub fn entity_count(&self) -> u64 {
        self.entity_count
    }

    pub fn index_base(&self) -> IndexBase {
        self.index_base
    }

    pub fn external_ids(&self) -> Option<&[i64]> {
        self.external_ids.as_deref()
    }
}

impl AdditiveField {
    pub fn domain(&self) -> &DomainId {
        &self.domain
    }

    pub fn field_id(&self) -> &str {
        &self.field_id
    }

    pub fn values(&self) -> &[f64] {
        &self.values
    }
}

/// Derive an unambiguous cardinality from one dense mapping side.
///
/// # Errors
///
/// Returns [`CardinalityError`] when the mapping side is empty or not dense,
/// contiguous, and zero-based.
#[instrument(skip(mapping))]
pub fn derive_cardinality(
    mapping: &MappingGeometry,
    side: MappingSide,
) -> Result<u64, CardinalityError> {
    let indices = mapping.indices_on(side).collect::<BTreeSet<_>>();
    let Some(&min) = indices.first() else {
        return Err(CardinalityError::EmptyMapping);
    };
    let max = *indices.last().ok_or(CardinalityError::EmptyMapping)?;
    let derived = (max + 1).max(0) as u64;

    if min != 0 || max < 0 || indices.len() != derived as usize {
        return Err(CardinalityError::NonDenseMapping {
            derived,
            distinct: indices.len(),
        });
    }

    Ok(derived)
}

/// Cross-check that a mapping covers exactly the descriptor's dense entity set.
///
/// # Errors
///
/// Returns [`CardinalityError`] when the domain is not in the mapping, an index is
/// out of range, or the dense derived count disagrees with manifest authority.
#[instrument(skip(descriptor, mapping))]
pub fn cross_check(
    descriptor: &DomainDescriptor,
    mapping: &MappingGeometry,
) -> Result<(), CardinalityError> {
    let domain = descriptor.domain().as_str().to_string();
    let side =
        mapping
            .side_for_domain(descriptor.domain())
            .ok_or_else(|| CardinalityError::DomainNotInMapping {
                domain: domain.clone(),
            })?;

    if let Some(index) = mapping
        .indices_on(side)
        .find(|&index| index < 0 || index as u64 >= descriptor.entity_count())
    {
        return Err(CardinalityError::DanglingEntityId {
            domain,
            index,
            entity_count: descriptor.entity_count(),
        });
    }

    let derived = derive_cardinality(mapping, side)?;
    if derived != descriptor.entity_count() {
        return Err(CardinalityError::CardinalityMismatch {
            domain,
            descriptor_count: descriptor.entity_count(),
            derived_count: derived,
        });
    }

    debug!(
        domain = descriptor.domain().as_str(),
        entity_count = descriptor.entity_count(),
        "mapping cross-check passed"
    );
    Ok(())
}

/// Sum one field across multiple same-domain attribute sources.
///
/// # Errors
///
/// Returns [`CardinalityError`] when any source has a non-dense `entity_index` or
/// no source contains `field_id`.
#[instrument(skip(descriptor, sources))]
pub fn project_additive_field(
    descriptor: &DomainDescriptor,
    field_id: &str,
    sources: &[DomainAttributes],
) -> Result<AdditiveField, CardinalityError> {
    let n = descriptor.entity_count() as usize;
    let mut values = vec![0.0_f64; n];
    let mut found = false;

    for source in sources {
        check_attribute_index(descriptor, source)?;
        if let Some(column) = source
            .attributes()
            .iter()
            .find(|column| column.field_id() == field_id)
        {
            found = true;
            for (&entity_index, &value) in source.entity_index().iter().zip(column.values()) {
                values[entity_index as usize] += value;
            }
        }
    }

    if !found {
        return Err(CardinalityError::MissingAdditiveField {
            domain: descriptor.domain().as_str().to_string(),
            field_id: field_id.to_string(),
        });
    }

    Ok(AdditiveField {
        domain: descriptor.domain().clone(),
        field_id: field_id.to_string(),
        values,
    })
}

fn check_attribute_index(
    descriptor: &DomainDescriptor,
    source: &DomainAttributes,
) -> Result<(), CardinalityError> {
    let indices = source.entity_index().iter().copied().collect::<BTreeSet<_>>();
    let dense = if descriptor.entity_count() == 0 {
        source.num_rows() == 0 && indices.is_empty()
    } else {
        indices.first() == Some(&0)
            && indices.last() == Some(&(descriptor.entity_count() as i64 - 1))
            && indices.len() == descriptor.entity_count() as usize
            && indices.len() == source.num_rows()
    };

    if !dense || source.entity_index().iter().any(|&index| index < 0) {
        return Err(CardinalityError::NonDenseAttributeIndex {
            domain: descriptor.domain().as_str().to_string(),
            entity_count: descriptor.entity_count(),
            distinct: indices.len(),
        });
    }

    Ok(())
}

/// Report OD4 cardinality, cross-check, and additive-field verdicts.
#[derive(Debug, thiserror::Error)]
pub enum CardinalityError {
    /// Fires when `external_ids` length does not equal authoritative entity_count.
    #[error(
        "domain {domain:?} external_ids length {external_ids_len} != entity_count {entity_count} (spec §5.4)"
    )]
    ExternalIdsLengthMismatch {
        domain: String,
        entity_count: u64,
        external_ids_len: usize,
    },

    /// Fires when a mapping side is not dense, contiguous, and zero-based.
    #[error(
        "mapping index set is not dense_zero_based: derived={derived} but {distinct} distinct indices"
    )]
    NonDenseMapping { derived: u64, distinct: usize },

    /// Fires when a mapping side carries no entries.
    #[error("mapping has no entries to derive a cardinality from")]
    EmptyMapping,

    /// Fires when derived mapping cardinality differs from manifest authority.
    #[error(
        "domain {domain:?} cardinality mismatch: manifest entity_count {descriptor_count} but mapping derives {derived_count}"
    )]
    CardinalityMismatch {
        domain: String,
        descriptor_count: u64,
        derived_count: u64,
    },

    /// Fires when a mapping index is outside the manifest domain's dense range.
    #[error("domain {domain:?} dangling entity id {index}: outside [0, {entity_count})")]
    DanglingEntityId {
        domain: String,
        index: i64,
        entity_count: u64,
    },

    /// Fires when the descriptor's domain is neither mapping source nor target.
    #[error("domain {domain:?} is not referenced by the mapping")]
    DomainNotInMapping { domain: String },

    /// Fires when a domain-attribute source has a non-dense entity_index.
    #[error(
        "domain {domain:?} attribute entity_index is not dense_zero_based: entity_count={entity_count} but {distinct} distinct indices"
    )]
    NonDenseAttributeIndex {
        domain: String,
        entity_count: u64,
        distinct: usize,
    },

    /// Fires when no source table contains the requested additive field.
    #[error("domain {domain:?} has no additive source table for field {field_id:?}")]
    MissingAdditiveField { domain: String, field_id: String },
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

    use crate::domains::{
        CardinalityError, DomainDescriptor, MappingSide, cross_check, derive_cardinality,
        project_additive_field,
    };
    use crate::mappings::MappingGeometry;
    use crate::readers::control_plane::{read_domain_attributes, read_domain_mapping};
    use crate::types::{ArtifactRole, Domain, DomainId, IndexBase, Mapping, MappingPurpose};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hmx-{tag}-{}-{}.parquet",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_batch(path: &PathBuf, schema: Arc<Schema>, batch: RecordBatch) {
        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, schema, None)
                .expect("parquet writer must construct");
            writer.write(&batch).expect("batch write must succeed");
            writer.close().expect("parquet writer close must succeed");
        }
        std::fs::write(path, buffer).expect("temp parquet write must succeed");
    }

    fn write_mapping(path: &PathBuf, target_index: Vec<i64>) {
        let len = target_index.len();
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("source_index", DataType::Int64, false),
            ArrowField::new("target_index", DataType::Int64, false),
            ArrowField::new("weight", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(
                    (0..len as i64).collect::<Vec<i64>>(),
                )),
                Arc::new(Int64Array::from(target_index)),
                Arc::new(Float64Array::from(vec![1.0; len])),
            ],
        )
        .expect("synthetic record batch must build");
        write_batch(path, schema, batch);
    }

    fn write_attributes(path: &PathBuf, entity_index: Vec<i64>, field_id: &str, values: Vec<f64>) {
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("entity_index", DataType::Int64, false),
            ArrowField::new(field_id, DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(entity_index)),
                Arc::new(Float64Array::from(values)),
            ],
        )
        .expect("synthetic record batch must build");
        write_batch(path, schema, batch);
    }

    fn glacier_domain(entity_count: u64, external_ids: Option<Vec<i64>>) -> Domain {
        Domain {
            id: DomainId::new("glacier"),
            entity_count,
            index_base: IndexBase::DenseZeroBased,
            external_ids,
        }
    }

    fn cell_to_glacier_declaration() -> Mapping {
        Mapping {
            purpose: MappingPurpose::CellToGlacier,
            source_domain: DomainId::new("cell"),
            target_domain: DomainId::new("glacier"),
            variable: None,
            artifact_role: ArtifactRole::new("mapping.cell_to_glacier"),
        }
    }

    fn read_synthetic_geometry(target_index: Vec<i64>) -> MappingGeometry {
        let path = temp_path("domain-mapping");
        write_mapping(&path, target_index);
        let rows = read_domain_mapping(&path).expect("mapping must read");
        std::fs::remove_file(&path).ok();
        MappingGeometry::from_declaration(&cell_to_glacier_declaration(), &rows)
    }

    fn real_cell_to_glacier_geometry() -> MappingGeometry {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/real-dudh/cell_to_glacier.parquet"
        );
        MappingGeometry::read(path, &cell_to_glacier_declaration()).expect("real mapping reads")
    }

    #[test]
    fn real_cell_to_glacier_derives_single_cardinality() {
        let geom = real_cell_to_glacier_geometry();
        assert_eq!(geom.num_entries(), 3864);

        let derived = derive_cardinality(&geom, MappingSide::Target).expect("dense");
        assert_eq!(derived, 58);
    }

    #[test]
    fn real_mapping_cross_checks_against_manifest_domain() {
        let geom = real_cell_to_glacier_geometry();
        let domain = glacier_domain(58, Some((1..=58).collect()));
        let descriptor = DomainDescriptor::from_domain(&domain).expect("valid domain");

        assert_eq!(descriptor.entity_count(), 58);
        assert_eq!(descriptor.external_ids().map(<[i64]>::len), Some(58));
        cross_check(&descriptor, &geom).expect("real mapping covers the glacier domain");
    }

    #[test]
    fn dangling_mapping_id_is_rejected() {
        let geom = read_synthetic_geometry(vec![0, 1, 2, 99]);
        let domain = glacier_domain(58, None);
        let descriptor = DomainDescriptor::from_domain(&domain).expect("valid domain");

        let error = cross_check(&descriptor, &geom).expect_err("dangling id must fail");
        assert!(matches!(
            error,
            CardinalityError::DanglingEntityId {
                index: 99,
                entity_count: 58,
                ..
            }
        ));
    }

    #[test]
    fn external_ids_length_mismatch_is_rejected() {
        let domain = glacier_domain(58, Some((1..=57).collect()));

        let error =
            DomainDescriptor::from_domain(&domain).expect_err("external id length must fail");
        assert!(matches!(
            error,
            CardinalityError::ExternalIdsLengthMismatch {
                entity_count: 58,
                external_ids_len: 57,
                ..
            }
        ));
    }

    #[test]
    fn under_covered_mapping_is_rejected() {
        let geom = read_synthetic_geometry(vec![0, 1, 2]);
        let domain = glacier_domain(4, None);
        let descriptor = DomainDescriptor::from_domain(&domain).expect("valid domain");

        let error = cross_check(&descriptor, &geom).expect_err("under coverage must fail");
        assert!(matches!(
            error,
            CardinalityError::CardinalityMismatch {
                descriptor_count: 4,
                derived_count: 3,
                ..
            }
        ));

        let gapped = read_synthetic_geometry(vec![0, 1, 3]);
        let error = derive_cardinality(&gapped, MappingSide::Target)
            .expect_err("internal gap must fail");
        assert!(matches!(
            error,
            CardinalityError::NonDenseMapping {
                derived: 4,
                distinct: 3
            }
        ));
    }

    #[test]
    fn additive_same_domain_field_sums_over_dense_index() {
        let domain = glacier_domain(3, None);
        let descriptor = DomainDescriptor::from_domain(&domain).expect("valid domain");
        let path_a = temp_path("domain-attributes-a");
        let path_b = temp_path("domain-attributes-b");
        write_attributes(&path_a, vec![0, 1, 2], "ice_volume", vec![1.0, 2.0, 3.0]);
        write_attributes(
            &path_b,
            vec![0, 1, 2],
            "ice_volume",
            vec![10.0, 20.0, 30.0],
        );
        let source_a = read_domain_attributes(&path_a).expect("attributes must read");
        let source_b = read_domain_attributes(&path_b).expect("attributes must read");
        std::fs::remove_file(&path_a).ok();
        std::fs::remove_file(&path_b).ok();

        let field = project_additive_field(&descriptor, "ice_volume", &[source_a, source_b])
            .expect("additive field must project");

        assert_eq!(field.domain().as_str(), "glacier");
        assert_eq!(field.field_id(), "ice_volume");
        assert_eq!(field.values(), &[11.0, 22.0, 33.0]);

        let error = project_additive_field(&descriptor, "snow_volume", &[])
            .expect_err("missing field must fail");
        assert!(matches!(
            error,
            CardinalityError::MissingAdditiveField { field_id, .. } if field_id == "snow_volume"
        ));
    }

    #[test]
    fn non_dense_attribute_index_is_rejected() {
        let domain = glacier_domain(3, None);
        let descriptor = DomainDescriptor::from_domain(&domain).expect("valid domain");
        let path = temp_path("domain-attributes-gap");
        write_attributes(&path, vec![0, 1, 3], "ice_volume", vec![1.0, 2.0, 3.0]);
        let source = read_domain_attributes(&path).expect("attributes must read");
        std::fs::remove_file(&path).ok();

        let error = project_additive_field(&descriptor, "ice_volume", &[source])
            .expect_err("non-dense index must fail");
        assert!(matches!(
            error,
            CardinalityError::NonDenseAttributeIndex {
                entity_count: 3,
                distinct: 3,
                ..
            }
        ));
    }

    #[test]
    fn domain_not_in_mapping_is_rejected() {
        let geom = read_synthetic_geometry(vec![0, 1, 2]);
        let domain = Domain {
            id: DomainId::new("lake"),
            entity_count: 3,
            index_base: IndexBase::DenseZeroBased,
            external_ids: None,
        };
        let descriptor = DomainDescriptor::from_domain(&domain).expect("valid domain");

        let error = cross_check(&descriptor, &geom).expect_err("absent domain must fail");
        assert!(matches!(error, CardinalityError::DomainNotInMapping { .. }));
    }
}
