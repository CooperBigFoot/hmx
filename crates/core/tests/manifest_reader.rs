use std::path::Path;

use hmx_core::manifest;
use hmx_core::manifest::Manifest;
use hmx_core::types::{ArtifactFormat, MappingPurpose};

#[test]
fn fixture_parses_from_json_and_package_root_read() {
    let json = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/valid-package/manifest.json"
    ));

    let manifest = Manifest::from_json(json).unwrap_or_else(|err| {
        panic!("fixture manifest must parse, got {err:?}");
    });
    assert_eq!(manifest.domains().len(), 2);
    let glacier = &manifest.domains()[1];
    assert_eq!(glacier.id.as_str(), "glacier");
    assert_eq!(glacier.entity_count, 3);
    assert_eq!(glacier.external_ids, Some(vec![1, 2, 2001]));
    assert_eq!(manifest.mappings().len(), 1);
    assert_eq!(
        manifest.mappings()[0].purpose,
        MappingPurpose::CellToGlacier
    );
    assert_eq!(manifest.artifacts().len(), 3);
    assert_eq!(manifest.artifacts()[0].format, ArtifactFormat::FieldRegistryV1);
    assert_eq!(manifest.crs().as_str(), "EPSG:32645");

    let from_file = manifest::read(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/valid-package"
    )))
    .unwrap_or_else(|err| panic!("fixture package root must read, got {err:?}"));
    assert_eq!(from_file, manifest);
}
