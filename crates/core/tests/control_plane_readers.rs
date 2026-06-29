use std::collections::BTreeSet;
use std::path::Path;

use hmx_core::readers::control_plane::{read_domain_attributes, read_domain_mapping};

// A4b materializes full rows by design (spec §7.1, the assembled-row path).
// This complements A4's metadata-only BULK readers; it does not change their
// metadata-only discipline for `cog`, `zarr`, or `gauge_long_v1`.

#[test]
fn reads_real_dudh_cell_to_glacier_mapping() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/real-dudh/cell_to_glacier.parquet"
    ));

    let mapping = read_domain_mapping(path).expect("real Dudh mapping must read");
    let distinct_targets = mapping
        .target_index()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    assert_eq!(mapping.num_rows(), 3864);
    assert!(mapping.target_index().iter().all(|&t| (0..=57).contains(&t)));
    assert!(mapping.source_index().iter().all(|&s| s >= 1));
    assert_eq!(distinct_targets.len(), 58);
}

#[test]
fn reads_real_dudh_glacier_attributes() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/real-dudh/glacier_thickness_m_we.parquet"
    ));

    let attributes = read_domain_attributes(path).expect("real Dudh attributes must read");
    let expected_entity_index = (0..58).collect::<Vec<i64>>();

    assert_eq!(attributes.entity_index(), expected_entity_index);
    assert_eq!(attributes.attributes().len(), 1);
    assert_eq!(
        attributes.attributes()[0].field_id(),
        "glacier.thickness_m_we"
    );
    assert_eq!(attributes.attributes()[0].values().len(), 58);
}

#[test]
fn real_dudh_rows_are_sufficient_for_later_cardinality_and_dangling_checks() {
    let mapping_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/real-dudh/cell_to_glacier.parquet"
    ));
    let attributes_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/real-dudh/glacier_thickness_m_we.parquet"
    ));

    let mapping = read_domain_mapping(mapping_path).expect("real Dudh mapping must read");
    let attributes = read_domain_attributes(attributes_path).expect("real Dudh attributes must read");
    let entity_count = attributes.num_rows();
    let distinct_targets = mapping
        .target_index()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    // A4b only surfaces these rows. Enforcing this cross-check is A6/A8.
    assert!(
        mapping
            .target_index()
            .iter()
            .all(|&t| t >= 0 && (t as usize) < entity_count)
    );
    assert_eq!(distinct_targets.len(), entity_count);
}
