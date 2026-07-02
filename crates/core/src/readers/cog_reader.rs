//! Cloud-Optimized GeoTIFF tag reader for COG metadata-only access.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::Decoder;
use tiff::tags::Tag;
use tracing::{debug, instrument};

use crate::CoreError;

const GEOKEY_GEOGRAPHIC_TYPE: u16 = 2048;
const GEOKEY_PROJECTED_TYPE: u16 = 3072;

/// Metadata recovered from a COG by reading TIFF tags only.
#[derive(Debug, Clone, PartialEq)]
pub struct CogMetadata {
    width: u32,
    height: u32,
    dtype: String,
    band_count: u16,
    crs_epsg: Option<u32>,
    pixel_scale: Option<(f64, f64)>,
}

impl CogMetadata {
    /// Returns the raster width from ImageWidth.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns the raster height from ImageLength.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns the sample dtype inferred from SampleFormat and BitsPerSample.
    pub fn dtype(&self) -> &str {
        &self.dtype
    }

    /// Returns the band count from SamplesPerPixel.
    pub fn band_count(&self) -> u16 {
        self.band_count
    }

    /// Returns the inline EPSG code from GeoKeyDirectory, when present.
    pub fn crs_epsg(&self) -> Option<u32> {
        self.crs_epsg
    }

    /// Returns the ModelPixelScale x/y magnitudes, when present.
    pub fn pixel_scale(&self) -> Option<(f64, f64)> {
        self.pixel_scale
    }
}

/// Opens a COG GeoTIFF and recovers dimensions, dtype, CRS, and pixel scale tags.
///
/// # Errors
///
/// Returns [`CoreError::CogRead`] when the TIFF cannot be opened or required
/// baseline tags cannot be decoded.
#[instrument(fields(path = %path.as_ref().display()))]
pub fn read_cog_metadata(path: impl AsRef<Path>) -> Result<CogMetadata, CoreError> {
    let path = path.as_ref();
    let artifact = path.display().to_string();
    let file = File::open(path).map_err(|e| CoreError::CogRead {
        artifact: artifact.clone(),
        detail: format!("artifact unreadable: {e}"),
    })?;
    let reader = BufReader::new(file);
    let mut decoder = Decoder::new(reader).map_err(|e| CoreError::CogRead {
        artifact: artifact.clone(),
        detail: format!("not a valid TIFF: {e}"),
    })?;

    let (width, height) = decoder.dimensions().map_err(|e| CoreError::CogRead {
        artifact: artifact.clone(),
        detail: format!("dimensions unreadable: {e}"),
    })?;
    let band_count = decoder
        .find_tag_unsigned(Tag::SamplesPerPixel)
        .map_err(|e| CoreError::CogRead {
            artifact: artifact.clone(),
            detail: format!("SamplesPerPixel tag unreadable: {e}"),
        })?
        .unwrap_or(1);
    let sample_format_values = decoder
        .find_tag_unsigned_vec(Tag::SampleFormat)
        .map_err(|e| CoreError::CogRead {
            artifact: artifact.clone(),
            detail: format!("SampleFormat tag unreadable: {e}"),
        })?
        .unwrap_or_else(|| vec![1]);
    let sample_format = homogeneous_tag_value(&artifact, "SampleFormat", &sample_format_values)?;
    let bits_per_sample_values =
        decoder
            .get_tag_u16_vec(Tag::BitsPerSample)
            .map_err(|e| CoreError::CogRead {
                artifact: artifact.clone(),
                detail: format!("BitsPerSample tag unreadable: {e}"),
            })?;
    let bits_per_sample =
        homogeneous_tag_value(&artifact, "BitsPerSample", &bits_per_sample_values)?;
    let dtype = geotiff_dtype(sample_format, bits_per_sample);
    let crs_epsg = decoder
        .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
        .ok()
        .and_then(|dir| epsg_from_geokey_directory(&dir).map(u32::from));
    let pixel_scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .ok()
        .and_then(|scale| match scale.as_slice() {
            [x, y, ..] => Some((*x, *y)),
            _ => None,
        });

    debug!(
        width,
        height,
        dtype = %dtype,
        band_count,
        crs_epsg = crs_epsg.unwrap_or(0),
        "read COG metadata"
    );

    Ok(CogMetadata {
        width,
        height,
        dtype,
        band_count,
        crs_epsg,
        pixel_scale,
    })
}

fn homogeneous_tag_value(artifact: &str, tag_name: &str, values: &[u16]) -> Result<u16, CoreError> {
    let Some(first) = values.first().copied() else {
        return Err(CoreError::CogRead {
            artifact: artifact.to_string(),
            detail: format!("{tag_name} tag unreadable: no values"),
        });
    };
    if values.iter().any(|value| *value != first) {
        return Err(CoreError::CogRead {
            artifact: artifact.to_string(),
            detail: format!("{tag_name} values differ across bands: {values:?}"),
        });
    }
    Ok(first)
}

fn geotiff_dtype(sample_format: u16, bits: u16) -> String {
    match (sample_format, bits) {
        (3, 32) => "f32",
        (3, 64) => "f64",
        (2, 32) => "i32",
        (2, 64) => "i64",
        (1, 8) => "u8",
        (1, 16) => "u16",
        (1, 32) => "u32",
        _ => "unknown",
    }
    .to_string()
}

fn epsg_from_geokey_directory(dir: &[u16]) -> Option<u16> {
    if dir.len() < 4 {
        return None;
    }
    let number_of_keys = dir[3] as usize;
    let mut geographic = None;
    let mut projected = None;
    for key_index in 0..number_of_keys {
        let base = 4 + key_index * 4;
        let Some(entry) = dir.get(base..base + 4) else {
            break;
        };
        let key_id = entry[0];
        let tag_location = entry[1];
        let value = entry[3];
        if tag_location != 0 {
            continue;
        }
        match key_id {
            GEOKEY_GEOGRAPHIC_TYPE => geographic = Some(value),
            GEOKEY_PROJECTED_TYPE => projected = Some(value),
            _ => {}
        }
    }
    projected.or(geographic)
}
