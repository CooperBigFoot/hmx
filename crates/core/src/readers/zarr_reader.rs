//! Zarr v3 group metadata reader with bounded 1-D coordinate chunk scans.

use std::io::Read;
use std::path::Path;

use ruzstd::StreamingDecoder;
use serde_json::Value;
use tracing::{debug, instrument};
use zarrs_metadata::v3::ArrayMetadataV3;

use crate::CoreError;

/// Metadata recovered from one Zarr v3 array member.
#[derive(Debug, Clone, PartialEq)]
pub struct ZarrArrayMeta {
    name: String,
    shape: Vec<u64>,
    dtype: String,
}

impl ZarrArrayMeta {
    /// Returns the array member name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the declared array shape.
    pub fn shape(&self) -> &[u64] {
        &self.shape
    }

    /// Returns the declared Zarr data type string.
    pub fn dtype(&self) -> &str {
        &self.dtype
    }
}

/// Decoded values from one 1-D coordinate chunk.
#[derive(Debug, Clone, PartialEq)]
pub enum ZarrCoordinateValues {
    /// Little-endian `int64` coordinate values.
    I64(Vec<i64>),
    /// Little-endian `float64` coordinate values.
    F64(Vec<f64>),
}

/// One decoded 1-D coordinate array.
#[derive(Debug, Clone, PartialEq)]
pub struct ZarrCoordinate {
    name: String,
    values: ZarrCoordinateValues,
}

impl ZarrCoordinate {
    /// Returns the coordinate array name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrows the decoded coordinate values.
    pub fn values(&self) -> &ZarrCoordinateValues {
        &self.values
    }
}

/// Metadata recovered from a Zarr v3 group without reading data chunks.
#[derive(Debug, Clone, PartialEq)]
pub struct ZarrMetadata {
    arrays: Vec<ZarrArrayMeta>,
    coordinates: Vec<ZarrCoordinate>,
}

impl ZarrMetadata {
    /// Borrows array metadata recovered from `zarr.json`.
    pub fn arrays(&self) -> &[ZarrArrayMeta] {
        &self.arrays
    }

    /// Borrows decoded 1-D coordinate chunks.
    pub fn coordinates(&self) -> &[ZarrCoordinate] {
        &self.coordinates
    }
}

/// Opens a Zarr v3 group and recovers array metadata plus 1-D coordinate values.
///
/// # Errors
///
/// Returns [`CoreError::ZarrRead`] when `zarr.json`, array metadata, or a 1-D
/// coordinate chunk cannot be read or decoded.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_zarr_metadata(path: impl AsRef<Path>) -> Result<ZarrMetadata, CoreError> {
    let store = path.as_ref();
    let artifact = store.display().to_string();
    let members = read_member_map(store, &artifact)?;

    let mut arrays = Vec::with_capacity(members.len());
    let mut coordinates = Vec::new();
    for (name, value) in members {
        let _typed: ArrayMetadataV3 =
            serde_json::from_value(value.clone()).map_err(|e| CoreError::ZarrRead {
                artifact: artifact.clone(),
                detail: format!("member {name:?} is not valid Zarr v3 array metadata: {e}"),
            })?;
        let shape = value
            .get("shape")
            .and_then(Value::as_array)
            .ok_or_else(|| CoreError::ZarrRead {
                artifact: artifact.clone(),
                detail: format!("member {name:?} has no shape array"),
            })?
            .iter()
            .map(|v| {
                v.as_u64().ok_or_else(|| CoreError::ZarrRead {
                    artifact: artifact.clone(),
                    detail: format!("member {name:?} carries a non-u64 shape value"),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let dtype = value
            .get("data_type")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::ZarrRead {
                artifact: artifact.clone(),
                detail: format!("member {name:?} has no string data_type"),
            })?
            .to_string();

        if shape.len() == 1
            && let Some(coord) = read_coordinate(store, &artifact, &name, &dtype)?
        {
            coordinates.push(coord);
        }
        arrays.push(ZarrArrayMeta { name, shape, dtype });
    }

    debug!(
        arrays = arrays.len(),
        coordinates = coordinates.len(),
        "read zarr metadata"
    );

    Ok(ZarrMetadata {
        arrays,
        coordinates,
    })
}

fn read_member_map(
    store: &Path,
    artifact: &str,
) -> Result<serde_json::Map<String, Value>, CoreError> {
    let root = store.join("zarr.json");
    let bytes = std::fs::read(&root).map_err(|e| CoreError::ZarrRead {
        artifact: artifact.to_string(),
        detail: format!("root zarr.json unreadable: {e}"),
    })?;
    let json: Value = serde_json::from_slice(&bytes).map_err(|e| CoreError::ZarrRead {
        artifact: artifact.to_string(),
        detail: format!("root zarr.json is not valid JSON: {e}"),
    })?;
    let metadata = json
        .get("consolidated_metadata")
        .and_then(|c| c.get("metadata"))
        .and_then(Value::as_object)
        .ok_or_else(|| CoreError::ZarrRead {
            artifact: artifact.to_string(),
            detail: "root zarr.json has no inline consolidated metadata map".to_string(),
        })?;
    Ok(metadata.clone())
}

fn read_coordinate(
    store: &Path,
    artifact: &str,
    name: &str,
    dtype: &str,
) -> Result<Option<ZarrCoordinate>, CoreError> {
    match dtype {
        "int64" => Ok(Some(ZarrCoordinate {
            name: name.to_string(),
            values: ZarrCoordinateValues::I64(read_coord_i64(store, artifact, name)?),
        })),
        "float64" => Ok(Some(ZarrCoordinate {
            name: name.to_string(),
            values: ZarrCoordinateValues::F64(read_coord_f64(store, artifact, name)?),
        })),
        _ => Ok(None),
    }
}

fn read_coord_i64(store: &Path, artifact: &str, coord: &str) -> Result<Vec<i64>, CoreError> {
    let bytes = read_coord_bytes(store, artifact, coord)?;
    if !bytes.len().is_multiple_of(8) {
        return Err(CoreError::ZarrRead {
            artifact: artifact.to_string(),
            detail: format!(
                "coordinate {coord:?} decoded to {} bytes, not a multiple of 8",
                bytes.len()
            ),
        });
    }
    Ok(bytes
        .chunks_exact(8)
        .map(|c| {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(c);
            i64::from_le_bytes(buf)
        })
        .collect())
}

fn read_coord_f64(store: &Path, artifact: &str, coord: &str) -> Result<Vec<f64>, CoreError> {
    let bytes = read_coord_bytes(store, artifact, coord)?;
    if !bytes.len().is_multiple_of(8) {
        return Err(CoreError::ZarrRead {
            artifact: artifact.to_string(),
            detail: format!(
                "coordinate {coord:?} decoded to {} bytes, not a multiple of 8",
                bytes.len()
            ),
        });
    }
    Ok(bytes
        .chunks_exact(8)
        .map(|c| {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(c);
            f64::from_le_bytes(buf)
        })
        .collect())
}

fn read_coord_bytes(store: &Path, artifact: &str, coord: &str) -> Result<Vec<u8>, CoreError> {
    let chunk = store.join(coord).join("c").join("0");
    let raw = std::fs::read(&chunk).map_err(|e| CoreError::ZarrRead {
        artifact: artifact.to_string(),
        detail: format!("coordinate {coord:?} chunk unreadable: {e}"),
    })?;
    debug!(
        coordinate = coord,
        bytes = raw.len(),
        "read 1-D coordinate chunk (c/0)"
    );

    match StreamingDecoder::new(std::io::Cursor::new(raw.as_slice())) {
        Ok(mut decoder) => {
            let mut decoded = Vec::new();
            decoder
                .read_to_end(&mut decoded)
                .map_err(|e| CoreError::ZarrRead {
                    artifact: artifact.to_string(),
                    detail: format!("coordinate {coord:?} chunk failed to decompress: {e}"),
                })?;
            Ok(decoded)
        }
        Err(_) => Ok(raw),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::readers::zarr_reader::{
        ZarrCoordinateValues, read_zarr_metadata,
    };

    fn fixture_store() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic-zarr")
    }

    #[test]
    fn parses_array_shapes_and_decodes_one_dimensional_coordinate() {
        let meta = read_zarr_metadata(fixture_store()).expect("fixture zarr metadata must read");

        let time = meta
            .arrays()
            .iter()
            .find(|a| a.name() == "time")
            .expect("time array metadata present");
        assert_eq!(time.shape(), &[3]);
        assert_eq!(time.dtype(), "int64");

        let elevation = meta
            .arrays()
            .iter()
            .find(|a| a.name() == "elevation")
            .expect("data array metadata present");
        assert_eq!(elevation.shape(), &[3, 2]);
        assert_eq!(elevation.dtype(), "float32");

        let coord = meta
            .coordinates()
            .iter()
            .find(|c| c.name() == "time")
            .expect("time coordinate decoded");
        assert_eq!(coord.values(), &ZarrCoordinateValues::I64(vec![0, 1, 2]));
    }
}
