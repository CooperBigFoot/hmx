//! Typed payload readers for HMX spec §7.
//!
//! Bulk readers surface schemas, row-group metadata, GeoTIFF tags, and bounded
//! 1-D coordinate chunks without decoding payloads. Small control JSON readers
//! decode and return typed values.

use std::path::Path;

use bytes::Bytes;

use crate::CoreError;

pub mod cog_reader;
pub mod control_plane;
pub mod geoparquet_reader;
pub mod parameter_scalars_reader;
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
