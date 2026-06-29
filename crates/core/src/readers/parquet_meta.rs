//! Apache Parquet footer/schema reader for metadata-only BULK access.

use std::path::Path;
use std::sync::Arc;

use arrow::datatypes::Schema;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::metadata::ParquetMetaData;
use tracing::{debug, instrument};

use crate::CoreError;
use crate::readers::read_file_bytes;

/// Metadata recovered from a parquet byte source with NO data page decoded.
#[derive(Debug, Clone)]
pub struct ParquetMetadata {
    schema: Arc<Schema>,
    file_metadata: Arc<ParquetMetaData>,
}

impl ParquetMetadata {
    /// Borrows the arrow schema decoded from the parquet footer.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Returns the number of row groups recorded in the parquet footer.
    pub fn num_row_groups(&self) -> usize {
        self.file_metadata.num_row_groups()
    }

    /// Returns the file row count recorded in the parquet footer.
    pub fn num_rows(&self) -> i64 {
        self.file_metadata.file_metadata().num_rows()
    }

    /// Returns the arrow column names in schema order.
    pub fn column_names(&self) -> Vec<&str> {
        self.schema.fields().iter().map(|f| f.name().as_str()).collect()
    }

    /// Returns the compression codec for one row-group column, if indexes exist.
    pub fn column_compression(&self, row_group: usize, col: usize) -> Option<Compression> {
        if row_group >= self.file_metadata.num_row_groups() {
            return None;
        }
        let row_group = self.file_metadata.row_group(row_group);
        if col >= row_group.num_columns() {
            return None;
        }
        Some(row_group.column(col).compression())
    }

    /// Returns a file-level key-value metadata entry from the parquet footer.
    pub fn key_value(&self, key: &str) -> Option<&str> {
        self.file_metadata
            .file_metadata()
            .key_value_metadata()?
            .iter()
            .find(|kv| kv.key == key)
            .and_then(|kv| kv.value.as_deref())
    }
}

/// Opens a parquet file and recovers its footer/schema metadata.
///
/// # Errors
///
/// Returns [`CoreError::ArtifactUnreadable`] when the path cannot be read and
/// [`CoreError::ParquetRead`] when the parquet footer/schema cannot be decoded.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_parquet_metadata(path: impl AsRef<Path>) -> Result<ParquetMetadata, CoreError> {
    let path = path.as_ref();
    let artifact = path.display().to_string();
    let bytes = read_file_bytes(path)?;

    let builder =
        ParquetRecordBatchReaderBuilder::try_new(bytes).map_err(|e| CoreError::ParquetRead {
            artifact: artifact.clone(),
            detail: e.to_string(),
        })?;
    let schema = Arc::clone(builder.schema());
    let file_metadata = Arc::clone(builder.metadata());

    debug!(
        columns = schema.fields().len(),
        row_groups = file_metadata.num_row_groups(),
        num_rows = file_metadata.file_metadata().num_rows(),
        "read parquet metadata"
    );

    Ok(ParquetMetadata {
        schema,
        file_metadata,
    })
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

    use crate::readers::parquet_meta::read_parquet_metadata;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hmx-{tag}-{}-{}.parquet",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_tiny_gauge_long(path: &PathBuf) {
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("timestep", DataType::Int64, false),
            ArrowField::new("gauge_id", DataType::Int64, false),
            ArrowField::new("value", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int64Array::from(vec![0, 1])),
                Arc::new(Int64Array::from(vec![7, 7])),
                Arc::new(Float64Array::from(vec![12.5, 13.0])),
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
    fn recovers_schema_row_groups_and_rows() {
        let path = temp_path("parquet-meta");
        write_tiny_gauge_long(&path);

        let meta = read_parquet_metadata(&path).expect("metadata must read");
        std::fs::remove_file(&path).ok();

        assert_eq!(meta.column_names(), vec!["timestep", "gauge_id", "value"]);
        assert!(meta.num_row_groups() >= 1);
        assert_eq!(meta.num_rows(), 2);
    }
}
