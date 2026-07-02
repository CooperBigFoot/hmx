//! Real Dudh fixture coverage for metadata-only BULK readers.
//!
//! The no-bulk-decode guarantee is structural: these reader APIs expose only
//! schema, row-group metadata, TIFF tags, and bounded coordinate metadata. There
//! is no row, pixel, geometry-byte, or data-chunk accessor to call.

use arrow::datatypes::DataType;

use hmx_core::readers::cog_reader::read_cog_metadata;
use hmx_core::readers::parquet_meta::read_parquet_metadata;

#[test]
fn real_dudh_dem_cog_surfaces_dimensions_and_epsg_from_tags() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/real-dudh/dem.tif");

    let meta = read_cog_metadata(path).expect("real Dudh DEM COG metadata must read");

    assert_eq!(meta.width(), 302);
    assert_eq!(meta.height(), 477);
    assert_eq!(meta.crs_epsg(), Some(32645));
}

#[test]
fn multiband_float32_cog_surfaces_dtype_and_band_count() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/multiband-float32.tif"
    );

    let meta = read_cog_metadata(path).expect("multi-band float32 COG metadata must read");

    assert_eq!(meta.width(), 4);
    assert_eq!(meta.height(), 4);
    assert_eq!(meta.dtype(), "f32");
    assert_eq!(meta.band_count(), 2);
}

#[test]
fn real_dudh_forcing_parquet_surfaces_schema_and_compression() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/real-dudh/forcing_cct.parquet"
    );

    let meta = read_parquet_metadata(path).expect("real Dudh forcing metadata must read");

    assert_eq!(
        meta.schema().field_with_name("timestep").unwrap().data_type(),
        &DataType::Int64
    );
    assert_eq!(
        meta.schema().field_with_name("gauge_id").unwrap().data_type(),
        &DataType::Int64
    );
    assert_eq!(
        meta.schema().field_with_name("value").unwrap().data_type(),
        &DataType::Float64
    );
    assert!(meta.num_row_groups() >= 1);
    let value_index = meta
        .column_names()
        .iter()
        .position(|name| *name == "value")
        .expect("value column is present");
    assert!(
        meta.column_compression(0, value_index).is_some(),
        "compression codec is surfaced from row-group metadata"
    );
}

#[test]
fn real_dudh_glacier_attribute_parquet_surfaces_schema_and_rows() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/real-dudh/glacier_thickness_m_we.parquet"
    );

    let meta = read_parquet_metadata(path).expect("real Dudh glacier metadata must read");

    assert_eq!(
        meta.schema()
            .field_with_name("entity_index")
            .unwrap()
            .data_type(),
        &DataType::Int64
    );
    assert!(
        meta.schema()
            .fields()
            .iter()
            .any(|field| field.name() != "entity_index" && field.data_type() == &DataType::Float64),
        "at least one Float64 attribute column is present"
    );
    assert!(meta.num_rows() > 0);
}
