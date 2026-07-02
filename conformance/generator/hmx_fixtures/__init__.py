"""Dev-only HMX conformance fixture generator."""

import logging
import os


def configure_logging() -> None:
    """Configure package logging from HMX_FIXTURES_LOG_LEVEL."""
    level_name = os.environ.get("HMX_FIXTURES_LOG_LEVEL", "INFO").upper()
    level = getattr(logging, level_name, logging.INFO)
    logging.basicConfig(level=level, format="%(levelname)s %(name)s: %(message)s")


def get_logger(name: str) -> logging.Logger:
    """Return a generator logger."""
    configure_logging()
    return logging.getLogger(f"hmx_fixtures.{name}")
