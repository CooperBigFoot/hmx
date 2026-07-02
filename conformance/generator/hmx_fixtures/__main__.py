"""Smoke-import every pinned generator dependency."""

from hmx_fixtures import get_logger


def main() -> None:
    """Import pinned dependencies so regenerate.sh fails before writing bytes."""
    import geopandas  # noqa: F401
    import numpy  # noqa: F401
    import pyarrow  # noqa: F401
    import rasterio  # noqa: F401
    import shapely  # noqa: F401

    get_logger("smoke").info("pinned dependencies import successfully")


if __name__ == "__main__":
    main()
