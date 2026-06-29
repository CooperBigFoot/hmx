//! GeoParquet footer reader for schema plus `geo` key-value metadata.

use std::path::Path;

use arrow::datatypes::Schema;
use serde_json::Value;
use tracing::{debug, instrument};

use crate::CoreError;
use crate::readers::parquet_meta::{ParquetMetadata, read_parquet_metadata};

const GEO_KEY: &str = "geo";
const GEOMETRY_COLUMN: &str = "geometry";
const EPSG_AUTHORITY: &str = "EPSG";

/// Metadata recovered from a GeoParquet file without reading WKB geometry bytes.
#[derive(Debug, Clone)]
pub struct GeoparquetMetadata {
    parquet: ParquetMetadata,
    geo_version: String,
    primary_column: String,
    crs: Option<String>,
    num_rows: i64,
}

impl GeoparquetMetadata {
    /// Borrows the arrow schema decoded from the parquet footer.
    pub fn schema(&self) -> &Schema {
        self.parquet.schema()
    }

    /// Returns the primary geometry column named by the `geo` footer block.
    pub fn primary_column(&self) -> &str {
        &self.primary_column
    }

    /// Returns the best-effort CRS string from the primary geometry column.
    pub fn crs(&self) -> Option<&str> {
        self.crs.as_deref()
    }

    /// Returns the GeoParquet metadata version string.
    pub fn geo_version(&self) -> &str {
        &self.geo_version
    }

    /// Returns the file row count recorded in parquet metadata.
    pub fn num_rows(&self) -> i64 {
        self.num_rows
    }

    /// Returns true when the arrow schema contains the conventional geometry column.
    pub fn has_geometry_column(&self) -> bool {
        self.schema().fields().iter().any(|f| f.name() == GEOMETRY_COLUMN)
    }
}

/// Opens a GeoParquet file and recovers schema plus parsed `geo` footer metadata.
///
/// # Errors
///
/// Returns parquet reader errors from [`read_parquet_metadata`] or
/// [`CoreError::GeoMetadataMalformed`] for a present but malformed `geo` block.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_geoparquet_metadata(
    path: impl AsRef<Path>,
) -> Result<GeoparquetMetadata, CoreError> {
    let path = path.as_ref();
    let artifact = path.display().to_string();
    let parquet = read_parquet_metadata(path)?;
    let geo = match parquet.key_value(GEO_KEY) {
        Some(geo) => parse_geo(&artifact, geo)?,
        None => ParsedGeo {
            version: String::new(),
            primary_column: String::new(),
            crs: None,
        },
    };
    let num_rows = parquet.num_rows();

    debug!(
        primary_column = %geo.primary_column,
        crs = geo.crs.as_deref().unwrap_or(""),
        num_rows,
        "read geoparquet metadata"
    );

    Ok(GeoparquetMetadata {
        parquet,
        geo_version: geo.version,
        primary_column: geo.primary_column,
        crs: geo.crs,
        num_rows,
    })
}

#[derive(Debug)]
struct ParsedGeo {
    version: String,
    primary_column: String,
    crs: Option<String>,
}

fn parse_geo(artifact: &str, geo: &str) -> Result<ParsedGeo, CoreError> {
    let json: Value = serde_json::from_str(geo).map_err(|e| CoreError::GeoMetadataMalformed {
        artifact: artifact.to_string(),
        detail: format!("`geo` metadata is not valid JSON: {e}"),
    })?;
    let version = required_str(artifact, &json, "version")?.to_string();
    let primary_column = required_str(artifact, &json, "primary_column")?.to_string();
    let crs = json
        .get("columns")
        .and_then(|c| c.get(&primary_column))
        .and_then(|c| c.get("crs"))
        .map(surface_crs);

    Ok(ParsedGeo {
        version,
        primary_column,
        crs,
    })
}

fn required_str<'a>(artifact: &str, json: &'a Value, key: &str) -> Result<&'a str, CoreError> {
    json.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::GeoMetadataMalformed {
            artifact: artifact.to_string(),
            detail: format!("`geo` metadata has no string `{key}`"),
        })
}

fn surface_crs(crs: &Value) -> String {
    if let Some(s) = crs.as_str() {
        return s.to_string();
    }
    if let Some(code) = epsg_code_from_projjson(crs) {
        return format!("{EPSG_AUTHORITY}:{code}");
    }
    crs.to_string()
}

fn epsg_code_from_projjson(crs: &Value) -> Option<String> {
    let id = crs.get("id")?;
    let authority = id.get("authority").and_then(Value::as_str)?;
    if authority != EPSG_AUTHORITY {
        return None;
    }
    match id.get("code")? {
        Value::Number(n) if n.is_u64() => n.as_u64().map(|c| c.to_string()),
        Value::Number(n) if n.is_i64() => n.as_i64().map(|c| c.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use arrow::array::{BinaryArray, Int64Array};
    use arrow::datatypes::{DataType, Field as ArrowField, Schema};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::ArrowWriter;
    use parquet::file::metadata::KeyValue;
    use parquet::file::properties::WriterProperties;

    use crate::readers::geoparquet_reader::read_geoparquet_metadata;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "hmx-geoparquet-{}-{}.parquet",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn surfaces_geo_footer_without_geometry_accessor() {
        let path = temp_path();
        let schema = Arc::new(Schema::new(vec![
            ArrowField::new("reach_id", DataType::Int64, false),
            ArrowField::new("geometry", DataType::Binary, false),
        ]));
        let geometry = BinaryArray::from_vec(vec![&b"\x01\x02"[..], &b"\x03"[..]]);
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int64Array::from(vec![10, 20])), Arc::new(geometry)],
        )
        .expect("synthetic geoparquet batch must build");
        let geo = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","crs":"EPSG:32645"}}}"#;
        let props = WriterProperties::builder()
            .set_key_value_metadata(Some(vec![KeyValue::new(
                "geo".to_string(),
                geo.to_string(),
            )]))
            .build();

        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, schema, Some(props))
                .expect("parquet writer must construct");
            writer.write(&batch).expect("batch write must succeed");
            writer.close().expect("parquet writer close must succeed");
        }
        std::fs::write(&path, buffer).expect("temp geoparquet write must succeed");

        let meta = read_geoparquet_metadata(&path).expect("metadata must read");
        std::fs::remove_file(&path).ok();

        assert_eq!(meta.primary_column(), "geometry");
        assert_eq!(meta.geo_version(), "1.1.0");
        assert!(meta.has_geometry_column());
        assert_eq!(meta.crs(), Some("EPSG:32645"));
        assert_eq!(meta.num_rows(), 2);
    }
}
