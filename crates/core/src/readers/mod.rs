//! Metadata-only BULK payload readers for HMX spec §7.1.
//!
//! These readers surface schemas, row-group metadata, GeoTIFF tags, and bounded
//! 1-D coordinate chunks. They do not decode parquet data pages, COG pixels,
//! GeoParquet geometry blobs, or multi-dimensional Zarr data chunks.

use std::path::Path;

use bytes::Bytes;

use crate::CoreError;

pub mod cog_reader;
pub mod geoparquet_reader;
pub mod parquet_meta;
pub mod zarr_reader;

fn read_file_bytes(path: &Path) -> Result<Bytes, CoreError> {
    std::fs::read(path)
        .map(Bytes::from)
        .map_err(|e| CoreError::ArtifactUnreadable {
            path: path.display().to_string(),
            detail: e.to_string(),
        })
}
