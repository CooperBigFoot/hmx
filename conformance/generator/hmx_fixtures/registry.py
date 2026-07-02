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
) -> dict[str, str]:
    """Build a nine-key field-registry entry."""
    return {
        "id": field_id,
        "domain": domain,
        "quantity": quantity,
        "units": units,
        "value_type": "f64",
        "time_meaning": time_meaning,
        "role": role,
        "conservation_class": conservation_class,
        "extent": "scalar",
    }


def registry(fields: list[dict[str, str]]) -> dict[str, object]:
    """Build a field registry document."""
    return {"registry_version": "1", "fields": fields}


def write_registry(path: Path, fields: list[dict[str, str]]) -> None:
    """Write deterministic field registry JSON."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(registry(fields), indent=2) + "\n", encoding="utf-8")
