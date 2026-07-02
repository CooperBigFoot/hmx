"""Build deterministic HMX manifest JSON."""

from pathlib import Path
import json

FORMAT_VERSION = "0.1"
CREATED_AT = "2026-06-29T00:00:00Z"
MANIFEST_FIELDS = [
    "format_version",
    "name",
    "created_at",
    "producer",
    "producer_version",
    "package_kind",
    "crs",
    "grid",
    "domains",
    "mappings",
    "artifacts",
]

GRID = {
    "crs": "EPSG:32645",
    "extent": {"xmin": 0.0, "ymin": 0.0, "xmax": 1000.0, "ymax": 1000.0},
    "cell_size": 250.0,
    "nx": 2,
    "ny": 2,
    "origin": "upper_left",
}

SHA_BY_ROLE = {
    "registry.fields": "00" * 32,
    "static.dem": "44" * 32,
    "static.layers": "aa" * 32,
    "mapping.cell_to_glacier": "11" * 32,
    "mapping.cell_to_reach": "55" * 32,
    "mapping.cell_to_gauge": "66" * 32,
    "attributes.glacier": "22" * 32,
    "attributes.reach": "77" * 32,
    "forcing.flow": "33" * 32,
    "forcing.gauge": "88" * 32,
    "metadata.gauge": "99" * 32,
}

SIZE_BY_ROLE = {
    "registry.fields": 512,
    "static.dem": 1024,
    "static.layers": 1024,
    "mapping.cell_to_glacier": 2498,
    "mapping.cell_to_reach": 2498,
    "mapping.cell_to_gauge": 2498,
    "attributes.glacier": 2498,
    "attributes.reach": 2498,
    "forcing.flow": 1024,
    "forcing.gauge": 2498,
    "metadata.gauge": 2498,
}


def artifact(
    role: str,
    path: str,
    format_name: str,
    **extra: object,
) -> dict[str, object]:
    """Build an artifact entry with fixed placeholder metadata."""
    item: dict[str, object] = {
        "role": role,
        "path": path,
        "format": format_name,
        "sha256": SHA_BY_ROLE[role],
        "size_bytes": SIZE_BY_ROLE[role],
    }
    item.update(extra)
    return item


def build_manifest(
    *,
    name: str,
    domains: list[dict[str, object]],
    mappings: list[dict[str, object]],
    artifacts: list[dict[str, object]],
) -> dict[str, object]:
    """Return a manifest dict in schema order."""
    return {
        "format_version": FORMAT_VERSION,
        "name": name,
        "created_at": CREATED_AT,
        "producer": "hmx-fixtures",
        "producer_version": "0.1.0",
        "package_kind": "input",
        "crs": "EPSG:32645",
        "grid": GRID,
        "domains": domains,
        "mappings": mappings,
        "artifacts": artifacts,
    }


def write_manifest(path: Path, manifest: dict[str, object]) -> None:
    """Write deterministic manifest JSON."""
    path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
