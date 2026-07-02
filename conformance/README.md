# HMX Conformance Suite

This directory is a dev-only conformance harness. The generated fixture data under
`conformance/valid/` and `conformance/invalid/` is ignored by git and regenerated
with:

```bash
PYTHON=python3.12 conformance/generator/regenerate.sh
```

The committed artifacts are the generator source and the golden CLI stdout files
under `conformance/goldens/`.

## Fixture Inventory

Valid fixtures:

- `valid/minimal`: registry JSON, COG static artifact, `cell_to_glacier`
  `parquet/domain_mapping_v1`, glacier `parquet/domain_attributes_v1`, and one
  plain Zarr v3 root. This intentionally keeps the only Zarr artifact in the
  suite.
- `valid/real-shape-basin`: the real-shape package covering COG,
  `cell_to_reach_v1`, `cell_to_gauge_v1`, `gauge_long_v1`,
  `gauge_metadata_v1`, `domain_mapping_v1`, `domain_attributes_v1`, and
  `geoparquet/reach_topology_v1`.

Invalid fixtures:

- `invalid/unknown-format-version`: structural exit 2, pins `M1`.
- `invalid/extra-manifest-field`: structural exit 2, pins `M2`.
- `invalid/missing-crs`: structural exit 2, pins `M3`.
- `invalid/cell-to-gauge-missing-variable`: structural exit 2, pins the mapping
  variable manifest rule.
- `invalid/dotdot-substring-path`: report exit 1, only `P1` fails.
- `invalid/malformed-registry`: report exit 1, only `R1` fails.
- `invalid/undeclared-attribute-field`: report exit 1, only `R2` fails.
- `invalid/dangling-mapping-id`: report exit 1, only `D1` fails.
- `invalid/mapping-role-non-mapping-format`: report exit 1, only `MAP1` fails.
- `invalid/missing-required-column`: report exit 1, only `F1` fails.

## Goldens

Goldens are exact process-boundary CLI stdout bytes: compact JSON from the Rust
serializer plus the CLI trailing newline. `HMX_BLESS=1 cargo test --test
conformance` rewrites them; a normal run byte-compares them. This deliberately
differs from hdx's parsed-`serde_json::Value` golden comparison because A11 keeps
all conformance testing outside `crates/core`.

## Determinism Contract

The generator uses fixed `created_at`, placeholder SHA-256 values, and fixed
placeholder `size_bytes`; it never hashes emitted artifact bytes or records their
real file length. Invalid report fixtures are shaped so the pinned check is the
only failing check. The Rust harness also runs the CLI from the repo root and
passes repo-relative package paths, so any path-bearing detail remains
machine-independent.

The Rust test skips when generated fixture data is absent. The trigger is the
presence of a generated fixture under `conformance/valid/`, specifically
`conformance/valid/minimal/manifest.json`.
