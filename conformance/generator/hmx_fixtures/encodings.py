"""Write real-shape HMX fixture encodings."""

from pathlib import Path
import json

import geopandas as gpd
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import rasterio
from rasterio.transform import from_origin
from shapely.geometry import LineString


def _write_table(path: Path, table: pa.Table) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(
        table,
        path,
        compression="NONE",
        use_dictionary=False,
        write_statistics=True,
        coerce_timestamps=None,
    )


def write_domain_mapping(path: Path, target_index: list[int] | None = None) -> None:
    """Write parquet/domain_mapping_v1."""
    targets = target_index if target_index is not None else [0, 1, 2]
    table = pa.table(
        {
            "source_index": pa.array([0, 1, 2], type=pa.int64()),
            "target_index": pa.array(targets, type=pa.int64()),
            "weight": pa.array([1.0, 1.0, 1.0], type=pa.float64()),
        }
    )
    _write_table(path, table)


def write_domain_attributes(path: Path, field_id: str = "glacier.thickness_m_we") -> None:
    """Write parquet/domain_attributes_v1."""
    table = pa.table(
        {
            "entity_index": pa.array([0, 1, 2], type=pa.int64()),
            field_id: pa.array([0.1, 0.2, 0.3], type=pa.float64()),
        }
    )
    _write_table(path, table)


def write_cell_to_reach(path: Path) -> None:
    """Write parquet/cell_to_reach_v1."""
    table = pa.table(
        {
            "cell_index": pa.array([0, 1, 2, 3], type=pa.int64()),
            "reach_id": pa.array([0, 0, 1, 1], type=pa.int64()),
            "weight": pa.array([1.0, 1.0, 1.0, 1.0], type=pa.float64()),
        }
    )
    _write_table(path, table)


def write_cell_to_gauge(path: Path) -> None:
    """Write parquet/cell_to_gauge_v1."""
    table = pa.table(
        {
            "cell_index": pa.array([0, 1, 2, 3], type=pa.int64()),
            "gauge_id": pa.array([0, 0, 1, 1], type=pa.int64()),
            "weight": pa.array([0.5, 0.5, 0.5, 0.5], type=pa.float64()),
        }
    )
    _write_table(path, table)


def write_gauge_long(path: Path, *, include_value: bool = True) -> None:
    """Write parquet/gauge_long_v1."""
    columns = {
        "timestep": pa.array([0, 1, 0, 1], type=pa.int64()),
        "gauge_id": pa.array([0, 0, 1, 1], type=pa.int64()),
    }
    if include_value:
        columns["value"] = pa.array([10.0, 11.0, 20.0, 21.0], type=pa.float64())
    _write_table(path, pa.table(columns))


def write_gauge_metadata(path: Path) -> None:
    """Write parquet/gauge_metadata_v1."""
    table = pa.table(
        {
            "gauge_id": pa.array([0, 1], type=pa.int64()),
            "x": pa.array([100.0, 700.0], type=pa.float64()),
            "y": pa.array([900.0, 400.0], type=pa.float64()),
            "z": pa.array([1200.0, 900.0], type=pa.float64()),
            "name": pa.array(["upper", "lower"], type=pa.string()),
        }
    )
    _write_table(path, table)


def write_reach_topology(path: Path) -> None:
    """Write geoparquet/reach_topology_v1."""
    path.parent.mkdir(parents=True, exist_ok=True)
    frame = gpd.GeoDataFrame(
        {
            "reach_id": np.array([0, 1], dtype="int64"),
            "downstream_reach_id": [1, None],
            "order_index": np.array([0, 1], dtype="int64"),
            "manning_n": np.array([0.035, 0.04], dtype="float64"),
            "width_m": np.array([4.0, 6.0], dtype="float64"),
            "slope": np.array([0.02, 0.01], dtype="float64"),
            "length_m": np.array([500.0, 600.0], dtype="float64"),
        },
        geometry=[
            LineString([(0.0, 1000.0), (500.0, 500.0)]),
            LineString([(500.0, 500.0), (1000.0, 0.0)]),
        ],
        crs="EPSG:32645",
    )
    frame["downstream_reach_id"] = frame["downstream_reach_id"].astype("Int64")
    frame.to_parquet(path, index=False)


def write_cog(path: Path) -> None:
    """Write a small tiled GeoTIFF/COG-readable raster."""
    path.parent.mkdir(parents=True, exist_ok=True)
    data = np.array([[1.0, 2.0], [3.0, 4.0]], dtype="float32")
    with rasterio.open(
        path,
        "w",
        driver="GTiff",
        height=2,
        width=2,
        count=1,
        dtype="float32",
        crs="EPSG:32645",
        transform=from_origin(0.0, 1000.0, 250.0, 250.0),
        tiled=True,
        blockxsize=16,
        blockysize=16,
    ) as dataset:
        dataset.write(data, 1)


def write_multiband_cog(path: Path) -> None:
    """Write a small two-band tiled GeoTIFF/COG-readable raster."""
    path.parent.mkdir(parents=True, exist_ok=True)
    data = np.array(
        [
            [[1.0, 2.0], [3.0, 4.0]],
            [[101.0, 102.0], [103.0, 104.0]],
        ],
        dtype="float32",
    )
    with rasterio.open(
        path,
        "w",
        driver="GTiff",
        height=2,
        width=2,
        count=2,
        dtype="float32",
        crs="EPSG:32645",
        transform=from_origin(0.0, 1000.0, 250.0, 250.0),
        tiled=True,
        blockxsize=16,
        blockysize=16,
    ) as dataset:
        dataset.write(data)


def write_zarr(path: Path) -> None:
    """Write a minimal Zarr v3 group root."""
    path.mkdir(parents=True, exist_ok=True)
    path.joinpath("zarr.json").write_text(
        json.dumps(
            {
                "zarr_format": 3,
                "node_type": "group",
                "consolidated_metadata": {"kind": "inline", "metadata": {}},
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
