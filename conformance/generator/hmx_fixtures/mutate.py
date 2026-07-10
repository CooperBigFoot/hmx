"""Derive one-mutation invalid fixtures from valid baselines."""

from enum import Enum
from pathlib import Path
import json
import shutil

import pyarrow as pa
import pyarrow.parquet as pq

from hmx_fixtures import get_logger
from hmx_fixtures.assertions import assert_invalid_diff
from hmx_fixtures.encodings import write_domain_mapping, write_gauge_long
from hmx_fixtures.manifest import write_manifest
from hmx_fixtures.parameter_scalars import write_parameter_scalars


class Invalid(Enum):
    """Closed invalid fixture set."""

    UNKNOWN_FORMAT_VERSION = ("unknown-format-version", "M1", 2, "minimal")
    EXTRA_MANIFEST_FIELD = ("extra-manifest-field", "M2", 2, "minimal")
    MISSING_CRS = ("missing-crs", "M3", 2, "minimal")
    CELL_TO_GAUGE_MISSING_VARIABLE = ("cell-to-gauge-missing-variable", "mapping-MUST", 2, "real-shape-basin")
    DOTDOT_SUBSTRING_PATH = ("dotdot-substring-path", "P1", 1, "minimal")
    MALFORMED_REGISTRY = ("malformed-registry", "R1", 1, "minimal")
    UNDECLARED_ATTRIBUTE_FIELD = ("undeclared-attribute-field", "R2", 1, "minimal")
    DANGLING_MAPPING_ID = ("dangling-mapping-id", "D1", 1, "minimal")
    MAPPING_ROLE_NON_MAPPING_FORMAT = ("mapping-role-non-mapping-format", "MAP1", 1, "real-shape-basin")
    MISSING_REQUIRED_COLUMN = ("missing-required-column", "F1", 1, "real-shape-basin")
    PARAMETER_SCALARS_UNKNOWN_FIELD = ("parameter-scalars-unknown-field", "PARAM1", 1, "parameter-scalars")
    PARAMETER_SCALARS_NON_PARAMETER_FIELD = ("parameter-scalars-non-parameter-field", "PARAM1", 1, "parameter-scalars")
    PARAMETER_SCALARS_DUPLICATE_RASTER_SOURCE = ("parameter-scalars-duplicate-raster-source", "PARAM1", 1, "parameter-scalars")

    @property
    def folder(self) -> str:
        return self.value[0]

    @property
    def pinned(self) -> str:
        return self.value[1]

    @property
    def exit_code(self) -> int:
        return self.value[2]

    @property
    def baseline(self) -> str:
        return self.value[3]


def _read_manifest(root: Path) -> dict[str, object]:
    return json.loads(root.joinpath("manifest.json").read_text(encoding="utf-8"))


def _rewrite_manifest(root: Path, manifest: dict[str, object]) -> None:
    write_manifest(root / "manifest.json", manifest)


def _artifact(manifest: dict[str, object], role: str) -> dict[str, object]:
    for artifact in manifest["artifacts"]:  # type: ignore[index]
        if artifact["role"] == role:
            return artifact
    raise RuntimeError(f"missing artifact {role}")


def derive_invalid(valid_root: Path, invalid_root: Path, invalid: Invalid) -> None:
    """Copy a baseline and apply exactly one invalid mutation."""
    log = get_logger("mutate")
    baseline = valid_root / invalid.baseline
    target = invalid_root / invalid.folder
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(baseline, target)

    expected = {"manifest.json"}
    manifest = _read_manifest(target)

    if invalid is Invalid.UNKNOWN_FORMAT_VERSION:
        manifest["format_version"] = "0.1"
        _rewrite_manifest(target, manifest)
    elif invalid is Invalid.EXTRA_MANIFEST_FIELD:
        manifest["glacier_count"] = 3
        _rewrite_manifest(target, manifest)
    elif invalid is Invalid.MISSING_CRS:
        del manifest["crs"]
        _rewrite_manifest(target, manifest)
    elif invalid is Invalid.CELL_TO_GAUGE_MISSING_VARIABLE:
        for mapping in manifest["mappings"]:  # type: ignore[index]
            if mapping["purpose"] == "cell_to_gauge":
                del mapping["variable"]
        _rewrite_manifest(target, manifest)
    elif invalid is Invalid.DOTDOT_SUBSTRING_PATH:
        artifact = _artifact(manifest, "static.dem")
        old = target / str(artifact["path"])
        artifact["path"] = "a..b/flow.tif"
        new = target / "a..b" / "flow.tif"
        new.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(old, new)
        _rewrite_manifest(target, manifest)
        expected = {"manifest.json", "static/dem.tif", "a..b/flow.tif"}
    elif invalid is Invalid.MALFORMED_REGISTRY:
        registry_path = target / "registry/fields.json"
        registry = json.loads(registry_path.read_text(encoding="utf-8"))
        registry["fields"][0]["role"] = "bogus"
        registry_path.write_text(json.dumps(registry, indent=2) + "\n", encoding="utf-8")
        expected = {"registry/fields.json"}
    elif invalid is Invalid.UNDECLARED_ATTRIBUTE_FIELD:
        path = target / "attributes/glacier.parquet"
        table = pq.read_table(path, partitioning=None)
        table = table.append_column(
            "glacier.undeclared",
            pa.array([1.0, 2.0, 3.0], type=pa.float64()),
        )
        pq.write_table(
            table,
            path,
            compression="NONE",
            use_dictionary=False,
            write_statistics=True,
            coerce_timestamps=None,
        )
        rewritten = pq.read_schema(path)
        assert "glacier.thickness_m_we" in rewritten.names
        assert "glacier.undeclared" in rewritten.names
        expected = {"attributes/glacier.parquet"}
    elif invalid is Invalid.DANGLING_MAPPING_ID:
        write_domain_mapping(target / "mappings/cell_to_glacier.parquet", [0, 1, 99])
        expected = {"mappings/cell_to_glacier.parquet"}
    elif invalid is Invalid.MAPPING_ROLE_NON_MAPPING_FORMAT:
        for mapping in manifest["mappings"]:  # type: ignore[index]
            if mapping["purpose"] == "cell_to_reach":
                mapping["artifact_role"] = "static.dem"
        _rewrite_manifest(target, manifest)
    elif invalid is Invalid.MISSING_REQUIRED_COLUMN:
        write_gauge_long(target / "forcing/gauge_long.parquet", include_value=False)
        expected = {"forcing/gauge_long.parquet"}
    elif invalid in {
        Invalid.PARAMETER_SCALARS_UNKNOWN_FIELD,
        Invalid.PARAMETER_SCALARS_NON_PARAMETER_FIELD,
        Invalid.PARAMETER_SCALARS_DUPLICATE_RASTER_SOURCE,
    }:
        scalars_path = target / "parameter/scalars.json"
        scalars = json.loads(scalars_path.read_text(encoding="utf-8"))
        additions = {
            Invalid.PARAMETER_SCALARS_UNKNOWN_FIELD: ("cell.unknown", 1.0),
            Invalid.PARAMETER_SCALARS_NON_PARAMETER_FIELD: ("cell.air_temperature_c", -2.0),
            Invalid.PARAMETER_SCALARS_DUPLICATE_RASTER_SOURCE: ("cell.spatial_coefficient", 0.75),
        }
        key, value = additions[invalid]
        scalars[key] = value
        write_parameter_scalars(scalars_path, scalars)
        expected = {"parameter/scalars.json"}
    else:
        raise AssertionError(invalid)

    assert_invalid_diff(baseline, target, expected)
    if invalid is Invalid.DOTDOT_SUBSTRING_PATH:
        dotted = target / "a..b" / "flow.tif"
        if not dotted.exists():
            raise AssertionError(f"{dotted} missing; F1 would fail collaterally")
    log.info("derived %s pins=%s exit=%d", invalid.folder, invalid.pinned, invalid.exit_code)
