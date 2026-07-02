"""Emit all HMX conformance fixtures and run self-assertions."""

from argparse import ArgumentParser
from pathlib import Path
import shutil

from hmx_fixtures import get_logger
from hmx_fixtures.assertions import assert_valid
from hmx_fixtures.encodings import (
    write_cell_to_gauge,
    write_cell_to_reach,
    write_cog,
    write_domain_attributes,
    write_domain_mapping,
    write_gauge_long,
    write_gauge_metadata,
    write_reach_topology,
    write_zarr,
)
from hmx_fixtures.manifest import artifact, build_manifest, write_manifest
from hmx_fixtures.mutate import Invalid, derive_invalid
from hmx_fixtures.registry import field, write_registry


def _reset(root: Path) -> None:
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)


def emit_minimal(root: Path) -> None:
    """Emit valid/minimal."""
    _reset(root)
    write_registry(root / "registry/fields.json", [field("glacier.thickness_m_we", "glacier", "thickness", "m w.e.")])
    write_cog(root / "static/dem.tif")
    write_domain_mapping(root / "mappings/cell_to_glacier.parquet")
    write_domain_attributes(root / "attributes/glacier.parquet")
    write_zarr(root / "forcing/flow.zarr")

    manifest = build_manifest(
        name="minimal",
        domains=[
            {"id": "cell", "entity_count": 4, "index_base": "dense_zero_based"},
            {"id": "glacier", "entity_count": 3, "index_base": "dense_zero_based", "external_ids": [1, 2, 2001]},
        ],
        mappings=[
            {
                "purpose": "cell_to_glacier",
                "source_domain": "cell",
                "target_domain": "glacier",
                "artifact_role": "mapping.cell_to_glacier",
            }
        ],
        artifacts=[
            artifact("registry.fields", "registry/fields.json", "hmx/field_registry_v1"),
            artifact("static.dem", "static/dem.tif", "cog", crs="EPSG:32645"),
            artifact("mapping.cell_to_glacier", "mappings/cell_to_glacier.parquet", "parquet/domain_mapping_v1"),
            artifact("attributes.glacier", "attributes/glacier.parquet", "parquet/domain_attributes_v1", domain="glacier"),
            artifact("forcing.flow", "forcing/flow.zarr", "zarr", variable="flow"),
        ],
    )
    write_manifest(root / "manifest.json", manifest)
    assert_valid(root, minimal=True)


def emit_real_shape_basin(root: Path) -> None:
    """Emit valid/real-shape-basin."""
    _reset(root)
    write_registry(
        root / "registry/fields.json",
        [
            field("glacier.thickness_m_we", "glacier", "thickness", "m w.e."),
            field("gauge.discharge", "gauge", "discharge", "m3 s-1", role="forcing", time_meaning="rate", conservation_class="water_volume"),
        ],
    )
    write_cog(root / "static/dem.tif")
    write_domain_mapping(root / "mappings/cell_to_glacier.parquet")
    write_domain_attributes(root / "attributes/glacier.parquet")
    write_cell_to_reach(root / "mappings/cell_to_reach.parquet")
    write_reach_topology(root / "topology/reaches.parquet")
    write_cell_to_gauge(root / "mappings/cell_to_gauge.parquet")
    write_gauge_long(root / "forcing/gauge_long.parquet")
    write_gauge_metadata(root / "metadata/gauge.parquet")

    manifest = build_manifest(
        name="real-shape-basin",
        domains=[
            {"id": "cell", "entity_count": 4, "index_base": "dense_zero_based"},
            {"id": "reach", "entity_count": 2, "index_base": "dense_zero_based"},
            {"id": "gauge", "entity_count": 2, "index_base": "dense_zero_based"},
            {"id": "glacier", "entity_count": 3, "index_base": "dense_zero_based", "external_ids": [1, 2, 2001]},
        ],
        mappings=[
            {
                "purpose": "cell_to_reach",
                "source_domain": "cell",
                "target_domain": "reach",
                "artifact_role": "mapping.cell_to_reach",
            },
            {
                "purpose": "cell_to_gauge",
                "source_domain": "cell",
                "target_domain": "gauge",
                "variable": "gauge.discharge",
                "artifact_role": "mapping.cell_to_gauge",
            },
            {
                "purpose": "cell_to_glacier",
                "source_domain": "cell",
                "target_domain": "glacier",
                "artifact_role": "mapping.cell_to_glacier",
            },
        ],
        artifacts=[
            artifact("registry.fields", "registry/fields.json", "hmx/field_registry_v1"),
            artifact("static.dem", "static/dem.tif", "cog", crs="EPSG:32645"),
            artifact("mapping.cell_to_reach", "mappings/cell_to_reach.parquet", "parquet/cell_to_reach_v1"),
            artifact("attributes.reach", "topology/reaches.parquet", "geoparquet/reach_topology_v1", domain="reach"),
            artifact("mapping.cell_to_gauge", "mappings/cell_to_gauge.parquet", "parquet/cell_to_gauge_v1"),
            artifact("forcing.gauge", "forcing/gauge_long.parquet", "parquet/gauge_long_v1", variable="gauge.discharge"),
            artifact("metadata.gauge", "metadata/gauge.parquet", "parquet/gauge_metadata_v1", domain="gauge"),
            artifact("mapping.cell_to_glacier", "mappings/cell_to_glacier.parquet", "parquet/domain_mapping_v1"),
            artifact("attributes.glacier", "attributes/glacier.parquet", "parquet/domain_attributes_v1", domain="glacier"),
        ],
    )
    write_manifest(root / "manifest.json", manifest)
    assert_valid(root, minimal=False)


def main() -> None:
    """Emit all fixtures."""
    parser = ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    args = parser.parse_args()
    repo_root = Path(args.repo_root)
    valid_root = repo_root / "conformance/valid"
    invalid_root = repo_root / "conformance/invalid"
    log = get_logger("build")

    emit_minimal(valid_root / "minimal")
    emit_real_shape_basin(valid_root / "real-shape-basin")
    for invalid in Invalid:
        derive_invalid(valid_root, invalid_root, invalid)

    log.info("emitted %d valid and %d invalid fixture(s)", 2, len(Invalid))
    print("hmx conformance fixtures regenerated")


if __name__ == "__main__":
    main()
