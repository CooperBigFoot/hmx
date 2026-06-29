//! Full-table materializer for small CONTROL parquets.

use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use tracing::{debug, instrument};

use crate::CoreError;
use crate::readers::read_file_bytes;

/// Every row of a SMALL control parquet, materialized into memory (the
/// assembled-row dispatch path, spec §7.1). Unlike the A4 BULK readers
/// (metadata only), control tables are read in FULL by design. This is the inert
/// row-bearing layer the typed projections (`DomainMapping`, `DomainAttributes`)
/// and the A6 mapping/cardinality logic are built on; it materializes, it does
/// not validate.
#[derive(Debug, Clone)]
pub struct ControlTable {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
    artifact: String,
}

impl ControlTable {
    /// Borrows the arrow schema decoded from the control parquet.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Returns the materialized row count across all record batches.
    pub fn num_rows(&self) -> usize {
        self.batches.iter().map(RecordBatch::num_rows).sum()
    }

    /// Returns the arrow column names in schema order.
    pub fn column_names(&self) -> Vec<&str> {
        self.schema.fields().iter().map(|f| f.name().as_str()).collect()
    }

    /// Materializes a required non-null `Int64` column across all batches.
    pub fn int64_column(&self, name: &str) -> Result<Vec<i64>, CoreError> {
        let mut values = Vec::with_capacity(self.num_rows());
        for batch in &self.batches {
            let column = batch
                .column_by_name(name)
                .and_then(|array| array.as_any().downcast_ref::<Int64Array>())
                .ok_or_else(|| self.malformed(format!("column `{name}` missing or not int64")))?;
            for index in 0..column.len() {
                if column.is_null(index) {
                    return Err(self.malformed(format!(
                        "null in required int64 column `{name}`"
                    )));
                }
                values.push(column.value(index));
            }
        }
        Ok(values)
    }

    /// Materializes a required non-null `Float64` column across all batches.
    pub fn float64_column(&self, name: &str) -> Result<Vec<f64>, CoreError> {
        let mut values = Vec::with_capacity(self.num_rows());
        for batch in &self.batches {
            let column = batch
                .column_by_name(name)
                .and_then(|array| array.as_any().downcast_ref::<Float64Array>())
                .ok_or_else(|| self.malformed(format!("column `{name}` missing or not float64")))?;
            for index in 0..column.len() {
                if column.is_null(index) {
                    return Err(self.malformed(format!(
                        "null in required float64 column `{name}`"
                    )));
                }
                values.push(column.value(index));
            }
        }
        Ok(values)
    }

    /// Materializes a nullable `Utf8` column across all batches.
    pub fn string_column(&self, name: &str) -> Result<Vec<Option<String>>, CoreError> {
        let mut values = Vec::with_capacity(self.num_rows());
        for batch in &self.batches {
            let column = batch
                .column_by_name(name)
                .and_then(|array| array.as_any().downcast_ref::<StringArray>())
                .ok_or_else(|| self.malformed(format!("column `{name}` missing or not utf8")))?;
            for index in 0..column.len() {
                values.push(if column.is_null(index) {
                    None
                } else {
                    Some(column.value(index).to_string())
                });
            }
        }
        Ok(values)
    }

    fn malformed(&self, detail: String) -> CoreError {
        CoreError::ControlTableMalformed {
            artifact: self.artifact.clone(),
            detail,
        }
    }
}

/// A fully-materialized `parquet/domain_mapping_v1` table: the parallel
/// `source_index` / `target_index` / `weight` columns of a generic source→target
/// cross-domain mapping (spec §7.1, §8). Rows are surfaced verbatim; cardinality
/// derivation, dense-index enforcement, and dangling-id detection are A6/A8.
#[derive(Debug, Clone)]
pub struct DomainMapping {
    source_index: Vec<i64>,
    target_index: Vec<i64>,
    weight: Vec<f64>,
}

impl DomainMapping {
    /// Borrows the source-domain entity indices.
    pub fn source_index(&self) -> &[i64] {
        &self.source_index
    }

    /// Borrows the target-domain entity indices.
    pub fn target_index(&self) -> &[i64] {
        &self.target_index
    }

    /// Borrows the mapping weights.
    pub fn weight(&self) -> &[f64] {
        &self.weight
    }

    /// Returns the materialized row count.
    pub fn num_rows(&self) -> usize {
        self.source_index.len()
    }
}

/// A fully-materialized `parquet/domain_attributes_v1` table: the dense
/// `entity_index` column plus every float64 attribute column, keyed by its
/// field id (spec §7.1, §5.5). Surfaced verbatim — multi-source additivity,
/// dense-zero-based verification, and registry cross-check are A5/A6/A8.
#[derive(Debug, Clone)]
pub struct DomainAttributes {
    entity_index: Vec<i64>,
    attributes: Vec<DomainAttributeColumn>,
}

impl DomainAttributes {
    /// Borrows the entity indices.
    pub fn entity_index(&self) -> &[i64] {
        &self.entity_index
    }

    /// Borrows the float64 attribute columns.
    pub fn attributes(&self) -> &[DomainAttributeColumn] {
        &self.attributes
    }

    /// Returns the materialized row count.
    pub fn num_rows(&self) -> usize {
        self.entity_index.len()
    }

    /// Returns attribute field ids in schema order.
    pub fn attribute_field_ids(&self) -> Vec<&str> {
        self.attributes
            .iter()
            .map(DomainAttributeColumn::field_id)
            .collect()
    }
}

/// One float64 attribute column of a `domain_attributes_v1` table.
#[derive(Debug, Clone)]
pub struct DomainAttributeColumn {
    field_id: String,
    values: Vec<f64>,
}

impl DomainAttributeColumn {
    /// Borrows the field id for this attribute column.
    pub fn field_id(&self) -> &str {
        &self.field_id
    }

    /// Borrows the materialized float64 values.
    pub fn values(&self) -> &[f64] {
        &self.values
    }
}

/// Opens a SMALL control parquet at `path` and materializes EVERY row into a
/// `ControlTable`. Reads the full table by design (spec §7.1); this is NOT the
/// metadata-only path.
///
/// # Errors
///
/// Returns [`CoreError::ArtifactUnreadable`] when the path cannot be read and
/// [`CoreError::ParquetRead`] when parquet decoding or batch iteration fails.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_control_table(path: impl AsRef<Path>) -> Result<ControlTable, CoreError> {
    let path = path.as_ref();
    let artifact = path.display().to_string();
    let bytes = read_file_bytes(path)?;

    let builder =
        ParquetRecordBatchReaderBuilder::try_new(bytes).map_err(|e| CoreError::ParquetRead {
            artifact: artifact.clone(),
            detail: e.to_string(),
        })?;
    let schema = Arc::clone(builder.schema());
    let reader = builder.build().map_err(|e| CoreError::ParquetRead {
        artifact: artifact.clone(),
        detail: e.to_string(),
    })?;
    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch.map_err(|e| CoreError::ParquetRead {
            artifact: artifact.clone(),
            detail: e.to_string(),
        })?);
    }

    let table = ControlTable {
        schema,
        batches,
        artifact,
    };
    debug!(
        rows = table.num_rows(),
        columns = ?table.column_names(),
        "materialized control table"
    );

    Ok(table)
}

/// Materializes a `parquet/domain_mapping_v1` table (`source_index`,
/// `target_index`, `weight`) in full.
///
/// # Errors
///
/// Returns [`CoreError`] when the parquet cannot be opened or its required
/// materialized columns are missing, wrong-typed, or null.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_domain_mapping(path: impl AsRef<Path>) -> Result<DomainMapping, CoreError> {
    let table = read_control_table(path)?;
    let source_index = table.int64_column("source_index")?;
    let target_index = table.int64_column("target_index")?;
    let weight = table.float64_column("weight")?;

    Ok(DomainMapping {
        source_index,
        target_index,
        weight,
    })
}

/// Materializes a `parquet/domain_attributes_v1` table (`entity_index` + every
/// float64 attribute column) in full.
///
/// # Errors
///
/// Returns [`CoreError`] when the parquet cannot be opened, `entity_index`
/// cannot be materialized, or the table carries no float64 attribute column.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_domain_attributes(path: impl AsRef<Path>) -> Result<DomainAttributes, CoreError> {
    let table = read_control_table(path)?;
    let entity_index = table.int64_column("entity_index")?;
    let mut attributes = Vec::new();

    for field in table.schema().fields() {
        if field.data_type() == &DataType::Float64 {
            let field_id = field.name().to_string();
            let values = table.float64_column(&field_id)?;
            attributes.push(DomainAttributeColumn { field_id, values });
        }
    }

    if attributes.is_empty() {
        return Err(CoreError::ControlTableMalformed {
            artifact: table.artifact,
            detail: "domain_attributes_v1 has no float64 attribute column".to_string(),
        });
    }

    Ok(DomainAttributes {
        entity_index,
        attributes,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use arrow::array::{Float64Array, Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field as ArrowField, Schema};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::ArrowWriter;

    use crate::CoreError;
    use crate::readers::control_plane::{
        read_control_table, read_domain_attributes, read_domain_mapping,
    };

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
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("source_index", DataType::Int64, false),
            ArrowField::new("target_index", DataType::Int64, false),
            ArrowField::new("weight", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2])),
                Arc::new(Int64Array::from(target_index)),
                Arc::new(Float64Array::from(vec![1.0, 0.5, 0.5])),
            ],
        )
        .expect("synthetic record batch must build");
        write_batch(path, schema, batch);
    }

    #[test]
    fn reads_mapping_round_trip() {
        let path = temp_path("control-mapping");
        write_mapping(&path, vec![0, 1, 1]);

        let mapping = read_domain_mapping(&path).expect("mapping must read");
        std::fs::remove_file(&path).ok();

        assert_eq!(mapping.source_index(), &[0, 1, 2]);
        assert_eq!(mapping.target_index(), &[0, 1, 1]);
        assert_eq!(mapping.weight(), &[1.0, 0.5, 0.5]);
        assert_eq!(mapping.num_rows(), 3);
    }

    #[test]
    fn surfaces_dangling_mapping_row_for_later_validation() {
        let path = temp_path("control-dangling");
        write_mapping(&path, vec![0, 99, 1]);

        let mapping = read_domain_mapping(&path).expect("dangling id must surface");
        std::fs::remove_file(&path).ok();

        let entity_count = 3usize;
        assert!(
            mapping
                .target_index()
                .iter()
                .any(|&t| t < 0 || t as usize >= entity_count)
        );
    }

    #[test]
    fn reads_attributes_round_trip() {
        let path = temp_path("control-attributes");
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("entity_index", DataType::Int64, false),
            ArrowField::new("glacier.thickness_m_we", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(vec![0, 1, 2])),
                Arc::new(Float64Array::from(vec![10.0, 20.0, 30.0])),
            ],
        )
        .expect("synthetic record batch must build");
        write_batch(&path, schema, batch);

        let attributes = read_domain_attributes(&path).expect("attributes must read");
        std::fs::remove_file(&path).ok();

        assert_eq!(attributes.entity_index(), &[0, 1, 2]);
        assert_eq!(attributes.attributes().len(), 1);
        assert_eq!(
            attributes.attributes()[0].field_id(),
            "glacier.thickness_m_we"
        );
        assert_eq!(attributes.attributes()[0].values(), &[10.0, 20.0, 30.0]);
    }

    #[test]
    fn surfaces_malformed_mapping() {
        let path = temp_path("control-malformed");
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("source_index", DataType::Int64, false),
            ArrowField::new("target_index", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(vec![0, 1])),
                Arc::new(Int64Array::from(vec![0, 1])),
            ],
        )
        .expect("synthetic record batch must build");
        write_batch(&path, schema, batch);

        let error = read_domain_mapping(&path).expect_err("missing weight must fail");
        std::fs::remove_file(&path).ok();

        assert!(matches!(error, CoreError::ControlTableMalformed { .. }));
    }

    #[test]
    fn reads_nullable_utf8_column() {
        let path = temp_path("control-utf8");
        let schema = Arc::new(Schema::new(vec![ArrowField::new(
            "domain",
            DataType::Utf8,
            true,
        )]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(StringArray::from(vec![
                Some("cell"),
                None,
                Some("glacier"),
            ]))],
        )
        .expect("synthetic record batch must build");
        write_batch(&path, schema, batch);

        let table = read_control_table(&path).expect("control table must read");
        std::fs::remove_file(&path).ok();

        assert_eq!(
            table.string_column("domain").expect("utf8 column must read"),
            vec![Some("cell".to_string()), None, Some("glacier".to_string())]
        );
    }
}
