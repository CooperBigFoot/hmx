"""Write deterministic HMX parameter scalar JSON."""

from pathlib import Path
import json


def write_parameter_scalars(path: Path, values: dict[str, float | list[float]]) -> None:
    """Write a deterministic parameter-scalars object."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(values, indent=2) + "\n", encoding="utf-8")
