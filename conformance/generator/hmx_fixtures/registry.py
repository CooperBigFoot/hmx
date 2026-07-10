"""Write deterministic HMX field registries."""

from pathlib import Path
import json


def field(
    field_id: str,
    domain: str,
    quantity: str,
    units: str,
    *,
    role: str = "parameter",
    time_meaning: str = "instant",
    conservation_class: str = "none",
    extent: str = "scalar",
    layer_count: int | None = None,
) -> dict[str, object]:
    """Build a nine-key field-registry entry."""
    item: dict[str, object] = {
        "id": field_id,
        "domain": domain,
        "quantity": quantity,
        "units": units,
        "value_type": "f64",
        "time_meaning": time_meaning,
        "role": role,
        "conservation_class": conservation_class,
        "extent": extent,
    }
    if layer_count is not None:
        item["layer_count"] = layer_count
    return item


def registry(fields: list[dict[str, object]]) -> dict[str, object]:
    """Build a field registry document."""
    return {"registry_version": "1", "fields": fields}


def write_registry(path: Path, fields: list[dict[str, object]]) -> None:
    """Write deterministic field registry JSON."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(registry(fields), indent=2) + "\n", encoding="utf-8")
