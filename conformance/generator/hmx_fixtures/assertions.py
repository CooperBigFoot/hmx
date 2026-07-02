"""Re-open generated HMX fixtures and assert load-bearing properties."""

from pathlib import Path
import filecmp
import json

import pyarrow as pa
import pyarrow.parquet as pq
import rasterio

from hmx_fixtures.manifest import MANIFEST_FIELDS


class AssertionFailed(RuntimeError):
    """Raised when generated fixtures violate the intended shape."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionFailed(message)


def read_json(path: Path) -> dict[str, object]:
    """Read an object JSON document."""
    return json.loads(path.read_text(encoding="utf-8"))


def _read_table(path: Path) -> pa.Table:
    return pq.read_table(path, partitioning=None)


def _artifact(manifest: dict[str, object], role: str) -> dict[str, object]:
    artifacts = manifest["artifacts"]
    assert isinstance(artifacts, list)
    for artifact in artifacts:
        assert isinstance(artifact, dict)
        if artifact["role"] == role:
            return artifact
    raise AssertionFailed(f"missing artifact role {role}")


def _require_columns(root: Path, manifest: dict[str, object], role: str, columns: dict[str, str]) -> None:
    artifact = _artifact(manifest, role)
    table = _read_table(root / str(artifact["path"]))
    for name, type_name in columns.items():
        _require(name in table.schema.names, f"{role}: missing {name}")
        field = table.schema.field(name)
        if type_name == "i64":
            _require(pa.types.is_int64(field.type), f"{role}.{name}: {field.type} != i64")
        elif type_name == "f64":
            _require(pa.types.is_float64(field.type), f"{role}.{name}: {field.type} != f64")
        elif type_name == "utf8":
            _require(pa.types.is_string(field.type), f"{role}.{name}: {field.type} != utf8")


def assert_valid(root: Path, *, minimal: bool) -> None:
    """Assert a valid fixture's writer-side contract."""
    manifest = read_json(root / "manifest.json")
    _require(list(manifest.keys()) == MANIFEST_FIELDS, f"{root}: manifest field order/shape drift")
    _require(manifest["format_version"] == "0.1", f"{root}: wrong format_version")
    _require(manifest["package_kind"] == "input", f"{root}: wrong package_kind")

    domains = {d["id"]: d for d in manifest["domains"]}  # type: ignore[index]
    _require("external_ids" not in domains["cell"], f"{root}: cell must not declare external_ids")
    for domain in ("reach", "gauge"):
        if domain in domains:
            _require("external_ids" not in domains[domain], f"{root}: {domain} must not declare external_ids")
    glacier = domains["glacier"]
    _require(len(glacier["external_ids"]) == glacier["entity_count"], f"{root}: glacier external_ids length")

    registry = read_json(root / "registry/fields.json")
    declared = {field["id"] for field in registry["fields"]}  # type: ignore[index]

    mapping_artifact = _artifact(manifest, "mapping.cell_to_glacier")
    mapping = _read_table(root / str(mapping_artifact["path"]))
    targets = mapping.column("target_index").to_pylist()
    expected = list(range(int(glacier["entity_count"])))
    _require(sorted(set(targets)) == expected, f"{root}: glacier target_index is not dense zero-based")

    attr_artifact = _artifact(manifest, "attributes.glacier")
    attributes = _read_table(root / str(attr_artifact["path"]))
    entity_index = attributes.column("entity_index").to_pylist()
    _require(entity_index == list(range(len(entity_index))), f"{root}: entity_index is not dense zero-based")
    for name in attributes.schema.names:
        if name != "entity_index":
            _require(name in declared, f"{root}: undeclared attribute field {name}")

    _require_columns(
        root,
        manifest,
        "mapping.cell_to_glacier",
        {"source_index": "i64", "target_index": "i64", "weight": "f64"},
    )
    _require_columns(root, manifest, "attributes.glacier", {"entity_index": "i64"})

    cog = _artifact(manifest, "static.dem")
    with rasterio.open(root / str(cog["path"])) as dataset:
        _require(dataset.is_tiled, f"{root}: COG is not tiled")
        _require(str(dataset.crs) == "EPSG:32645", f"{root}: COG CRS drift")
        _require(dataset.transform is not None, f"{root}: COG missing transform")

    if minimal:
        zarr = _artifact(manifest, "forcing.flow")
        root_json = read_json(root / str(zarr["path"]) / "zarr.json")
        _require("consolidated_metadata" in root_json, f"{root}: zarr root is not consolidated")
    else:
        _require_columns(root, manifest, "mapping.cell_to_reach", {"cell_index": "i64", "reach_id": "i64", "weight": "f64"})
        _require_columns(root, manifest, "mapping.cell_to_gauge", {"cell_index": "i64", "gauge_id": "i64", "weight": "f64"})
        _require_columns(root, manifest, "forcing.gauge", {"timestep": "i64", "gauge_id": "i64", "value": "f64"})
        _require_columns(root, manifest, "metadata.gauge", {"gauge_id": "i64", "x": "f64", "y": "f64", "z": "f64", "name": "utf8"})
        _require_columns(
            root,
            manifest,
            "attributes.reach",
            {
                "reach_id": "i64",
                "downstream_reach_id": "i64",
                "order_index": "i64",
                "manning_n": "f64",
                "width_m": "f64",
                "slope": "f64",
                "length_m": "f64",
            },
        )


def relative_files(root: Path) -> set[str]:
    """Return relative file paths under root."""
    return {
        str(path.relative_to(root))
        for path in root.rglob("*")
        if path.is_file()
    }


def changed_files(left: Path, right: Path) -> set[str]:
    """Return files added, removed, or byte-changed between two trees."""
    names = relative_files(left) | relative_files(right)
    changed: set[str] = set()
    for name in names:
        a = left / name
        b = right / name
        if not a.exists() or not b.exists() or not filecmp.cmp(a, b, shallow=False):
            changed.add(name)
    return changed


def assert_invalid_diff(baseline: Path, invalid: Path, expected: set[str]) -> None:
    """Assert an invalid differs from its baseline only in expected files."""
    actual = changed_files(baseline, invalid)
    _require(actual == expected, f"{invalid}: changed files {sorted(actual)} != {sorted(expected)}")
