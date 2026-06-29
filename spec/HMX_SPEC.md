# HMX — Hydrology Model Exchange (`format_version` `0.1`)

> Normative specification (M13 / step A2). HMX is a prescriptive,
> friction-anchored interface for single-basin hydrology **model input**
> packages. Requirement keywords (`MUST`, `MUST NOT`, `SHALL`, `SHOULD`, `MAY`)
> are used per RFC 2119. The committed JSON Schemas under `schemas/` are the
> machine-checkable form of this document; on any conflict the schema and this
> document MUST agree (a divergence is a bug, fixed under the §14 revision rule).

## 0. Reading order & versioning discipline

0.1 A reader MUST read `format_version` FIRST and reject the package outright if
its value is not a recognized HMX format version. The only recognized value is
`"0.1"`. This is a HARD CUT: an unknown `format_version` MUST NOT be softened to
a validation report — it is a structural rejection (the CLI exit-code `2`, §10).

0.2 The on-disk manifest file is named `manifest.json` and lives at the package
root (decision OD13, §13). It is a UTF-8 JSON object conforming to
`schemas/manifest.schema.json`.

0.3 The describe/validate output shapes (§10) are versioned IMPLICITLY by
`format_version`; there is no separate output-schema-version field. A shape
change requires a `format_version` bump.

## 1. What HMX is — and is not

1.1 HMX describes ONE basin per package (a single hydrology-model input
package). Multi-basin partitioning is OUT of scope.

1.2 HMX is a STANDALONE SIBLING of HDX (Hydrology Dataset Exchange) and HFX. It
is NOT compositional with them and MUST NOT be defined as consuming HDX/HFX as
sub-layers, nor as a superset of either.

1.3 HMX formalizes the proven bluesmith package model; it does not invent a new
data encoding. The on-disk artifact formats (§7) reuse the existing bluesmith
COG / parquet / geoparquet shapes (decision OD5, §13).

1.4 OUT of scope for HMX `0.1` (a producer MUST NOT rely on HMX to carry these):
docs-for-AI generation, run dispatch / run-output packaging, evaluation /
leaderboard / metrics, and the run-output manifest (that remains a separate,
non-HMX bluesmith artifact).

## 2. Package layout

2.1 A package is a directory whose root MUST contain `manifest.json`. Every other
artifact is declared in the manifest's `artifacts[]` and stored at the declared
package-relative `path`.

2.2 Artifact paths MUST be package-relative: they MUST NOT begin with `/` and
MUST NOT contain a `..` parent-traversal segment (this keeps the package
relocatable and the content-hash path-independent, §9; prevents F1-class
absolute-path leakage).

2.3 The conventional sub-directories are `static/`, `parameter/`, `forcing/`,
`state/`, `attributes/`, `mappings/`, `topology/`, `gauges/`, and `registry/`.
These are conventions; the manifest's declared `path` is authoritative.

## 3. The manifest (`manifest.json`)

3.1 The manifest is a JSON object with EXACTLY these top-level keys (no more —
`additionalProperties:false`; no fewer — all are required): `format_version`,
`name`, `created_at`, `producer`, `producer_version`, `package_kind`, `crs`,
`grid`, `domains`, `mappings`, `artifacts`.

3.2 The manifest MUST NOT carry the content-hash. The content-hash is COMPUTED
from the manifest by `hmx-core` (§9), never stored in it (a stored hash would be
circular). The manifest MUST NOT carry any out-of-band entity-count field (e.g.
a top-level `glacier_count`); `additionalProperties:false` rejects it. Entity
cardinality lives ONLY in `domains[].entity_count` (§5; prevents F2/F10).

3.3 `format_version` MUST be `"0.1"`. `package_kind` MUST be `"input"` (HMX `0.1`
describes input packages only).

3.4 `name` is the package identity string (non-empty). `created_at` is an RFC
3339 date-time. `producer` / `producer_version` identify the writing tool.

## 4. CRS — explicit per package

4.1 The manifest MUST carry a non-empty top-level `crs` string. There MUST NOT be
a module-constant or implied CRS (prevents F1: the hardcoded `EPSG:32632`
wrangle constant that mislabeled the Nepal `EPSG:32645` Dudh package).

4.2 `grid.crs` MUST be present and SHOULD equal the package `crs`. A raster /
geo artifact MAY carry an OPTIONAL per-artifact `crs` override; when absent the
package `crs` applies.

## 5. Domains & entity cardinality

5.1 The manifest MUST declare every entity domain it carries in `domains[]`
(`minItems: 1`). Each domain object has EXACTLY `id`, `entity_count`,
`index_base`, and an optional `external_ids` (`additionalProperties:false`).

5.2 `entity_count` is the SINGLE authoritative cardinality of the domain
(decision OD4, §13). A consumer MUST take the cardinality from this field and
MUST NOT re-derive a second, possibly-conflicting count from elsewhere. This
eliminates the glacier `58 / 29 / 31` ambiguity (prevents F2/F10).

5.3 `index_base` MUST be `"dense_zero_based"`: entities are indexed `0 .. entity_count-1`
densely, matching the on-disk `entity_index` / `source_index` / `target_index`
columns (§7).

5.4 A domain MAY carry `external_ids`: an ordered integer array mapping internal
index `i` to the producer's external identifier (e.g. a glacier `Glac_ID`). When
present, `external_ids.length` MUST equal `entity_count`, and the validator
(A6/A8) MUST cross-check that the distinct entity indices derivable from the
domain's mapping geometry equal `0 .. entity_count-1` (prevents F9/F16 —
underivable glacier external ids). `external_ids` is intended for small domains
whose external identity is not otherwise on disk; large domains (cell, reach,
gauge) carry their external ids in their attribute/metadata tables and omit it.

5.5 Multi-source additive fields on a domain (e.g. glacier-state fields stored as
`domain_attributes_v1` tables that ride the same domain) MUST each be declared as
their own `artifacts[]` entry with the owning `domain`; they are additive over
the domain's dense index (prevents F3/F10).

## 6. The typed field registry

6.1 A package MUST declare exactly one field-registry artifact: an `artifacts[]`
entry with role `registry.fields` and format `hmx/field_registry_v1`, whose `path`
points to a JSON file conforming to `schemas/field_registry.schema.json`
(decision OD3 — the registry is a SEPARATE validated artifact, §13).

6.2 The registry lists every model-consumed and model-produced field. Each entry
has EXACTLY `id`, `domain`, `quantity`, `units`, `value_type`, `time_meaning`,
`role`, `conservation_class`, `extent` (`additionalProperties:false`).

6.3 `role` is the semantic role, one of `differential_state`, `parameter`,
`forcing`, `coupling`, `diagnostic`. `value_type` is one of `f32`, `f64`, `i32`,
`i64`, `bool`. `time_meaning` is one of `instant`, `rate`, `step_amount`.
`extent` is `scalar` or `per_layer`.

6.4 `conservation_class` is a FIRST-CLASS registry attribute (decision OD3), one
of `water_volume`, `energy`, `none`. It is declared explicitly and MUST NOT be
inferred from anything else. Non-conserved fields (parameters, diagnostics) use
`none`.

6.5 Every field a model consumes MUST be declarable in the registry, and a
consumer MUST reject a model-consumed field that is absent from the registry — no
undeclarable field may bypass the input-completeness gate (prevents F8/F19; the
`channel.section_shape` "no canonical role" gap).

## 7. On-disk artifact formats (the closed `format` set)

7.1 Every `artifacts[]` entry's `format` MUST be one of the closed set below.
HMX `0.1` reuses the proven bluesmith encodings verbatim (decision OD5, §13). A
reader for the BULK payloads (`cog`, `zarr`, and the large `parquet/gauge_long_v1`
forcing tables) MUST read metadata only (tags / row-group statistics / schema /
bounded 1-D coordinate scans) and MUST NOT decode a data chunk, pixel, or
geometry blob. The small CONTROL tables (`domain_mapping_v1`,
`domain_attributes_v1`, `cell_to_reach_v1`, `cell_to_gauge_v1`,
`gauge_metadata_v1`) are read in FULL by design (the assembled-row dispatch path).

| `format` | Encoding | Required columns (name : arrow type) |
|---|---|---|
| `cog` | Cloud-Optimized GeoTIFF, single band on the package grid | (raster; metadata/tags only) |
| `zarr` | Zarr v3 gridded payload (optional) | (gridded; `zarr.json` + 1-D coords only) |
| `geoparquet/reach_topology_v1` | GeoParquet (WKB LineString + `geo` metadata) | `reach_id`:int64, `downstream_reach_id`:int64?, `order_index`:int64, `manning_n`:float64, `width_m`:float64, `slope`:float64, `length_m`:float64, `geometry`:binary(WKB) |
| `parquet/gauge_long_v1` | Long-format forcing time series | `timestep`:int64, `gauge_id`:int64, `value`:float64 |
| `parquet/gauge_metadata_v1` | Gauge attribute table | `gauge_id`:int64, `x`:float64, `y`:float64, `z`:float64, `name`:utf8, [`source_gauge_id`:int64], [`source_gauge_code`:utf8] |
| `parquet/cell_to_gauge_v1` | Cell→gauge per-variable mapping | `cell_index`:int64, `gauge_id`:int64, `weight`:float64 |
| `parquet/cell_to_reach_v1` | Cell→reach mapping | `cell_index`:int64, `reach_id`:int64, `weight`:float64, [`source_cell_id`:int64] |
| `parquet/domain_attributes_v1` | Dense per-entity attribute table | `entity_index`:int64 (dense zero-based) + one float64 column per attribute field |
| `parquet/domain_mapping_v1` | Generic source→target mapping | `source_index`:int64, `target_index`:int64, `weight`:float64 |
| `hmx/field_registry_v1` | JSON, conforms to `field_registry.schema.json` | (JSON; §6) |

7.2 An `artifacts[]` entry has EXACTLY `role`, `path`, `format`, `sha256`
(required) plus the optional `size_bytes`, `crs`, `domain`, `variable`, `unit`,
`time_meaning`, `interval_seconds`, `row_count`, `first_step_index`,
`last_step_index` (`additionalProperties:false`). `sha256` is the lowercase-hex
SHA-256 of the artifact bytes (64 hex chars).

## 8. Cross-domain mappings

8.1 Cross-domain mappings are declared EXPLICITLY in `mappings[]`, not parsed out
of a role string (decision OD5/D3). Each mapping object has EXACTLY `purpose`,
`source_domain`, `target_domain`, `artifact_role`, and an optional `variable`
(`additionalProperties:false`).

8.2 `purpose` MUST be one of `cell_to_reach`, `cell_to_glacier`, `glacier_to_cell`,
`cell_to_gauge`. When `purpose` is `cell_to_gauge` the mapping MUST carry
`variable` (the forced variable the gauge mapping serves, e.g. `precipitation`).

8.3 `artifact_role` MUST reference an `artifacts[]` entry whose `format` is the
mapping's on-disk encoding (`parquet/domain_mapping_v1`, `parquet/cell_to_reach_v1`,
or `parquet/cell_to_gauge_v1`, §7).

## 9. The manifest content-hash

9.1 The package content-hash is COMPUTED by `hmx-core` (step A7), ONCE, and
exposed via the core API and surfaced by `describe` (§10). This document fixes
its SEMANTICS; A7 fixes the exact canonical byte sequence (its producer-side
definition, decision OD2-producer).

9.2 The content-hash MUST be path-independent and reproducible: hashing the same
logical package twice MUST yield the same SHA-256; a change confined to absolute
filesystem location, on-disk file ordering, or insignificant manifest whitespace
MUST NOT change it; changing any declared artifact `sha256` MUST change it.

9.3 The hash is computed over the canonical JSON serialization of the manifest
(keys sorted, no insignificant whitespace, RFC-8785-style) together with the
ordered set of declared artifact `sha256` digests. The exact canonical-byte
definition is A7's; consumers (bluesmith, anvil) record the hmx-core value and
MUST NOT recompute it independently.

## 10. Tooling — the contract-executing verbs

10.1 HMX defines two CLI verbs (implemented A8/A9): `describe` and `validate`.
Both emit JSON to stdout. Exit codes: `0` = conformant / success, `1` =
non-conformant (a MUST that ran failed), `2` = structural error (e.g. the §0 hard
version cut, unreadable manifest).

10.2 `describe` emits a facts-only self-description conforming to
`schemas/describe.schema.json`: the manifest identity floor, the package
content-hash (§9), the CRS, the grid, the domains, the field catalog, the
mappings, and the artifacts. It reports facts, never a verdict (no `conformant`
key).

10.3 `validate` emits `schemas/validate.schema.json`: `{checks, conformant}`.
`conformant` is true iff no check that RAN failed (fail-closed; a skip never
flips it). A violated MUST is carried as a `result: "fail"` outcome with
`conformant: false`, NEVER as a raised error.

10.4 The closed set of check ids, the severity model, and whether `--strict` is
supported are NOT fixed by this document — they are owned by step A8 (decision
OD6). `validate.schema.json` therefore constrains the OUTPUT WIRE SHAPE only and
leaves `id` as a non-empty string. A8 MAY file a one-field revision (§14) to pin
`id` to the closed enum or to add a `severity` field.

## 11. Conformance — the MUST checklist (validator scope)

11.1 A validator MUST check, at minimum: the §0 `format_version` hard cut; the §3
manifest shape; the §4 CRS presence; the §5 single-source `entity_count` and (when
present) the `external_ids.length == entity_count` + dense-index cross-check; the
§6 field-registry presence + every-model-consumed-field-declared gate; the §7
closed `format` set and per-format column shapes (metadata-deep); and the §8
mapping declarations resolving to real artifacts.

11.2 Conformance validates FORMAT only. It MUST NOT include any evaluation,
metric, leaderboard, or wall-time judgment (OUT of scope, §1.4).

## 12. Scope boundary — recap

HMX `0.1` carries: a single-basin model-input manifest, a typed field registry,
explicit multi-entity domains with one authoritative cardinality, explicit
cross-domain mappings, the reused bluesmith on-disk formats, an explicit CRS, and
a computed content-hash. It does NOT carry: compositional HDX/HFX layering,
multi-basin partitioning, docs-for-AI, run dispatch / run-output packaging, or
any evaluation layer.

## 13. Resolved design decisions

- **OD13 — manifest filename:** `manifest.json` (JSON, package root; §0.2). The
  bluesmith `manifest.toml` path-literal co-move that this implies is owned by
  M13 B-stage, not by this spec.
- **OD3 — field-registry placement + conservation-class:** the registry is a
  SEPARATE validated artifact (`registry/fields.json`, role `registry.fields`,
  format `hmx/field_registry_v1`); `conservation_class` is a first-class registry
  attribute, a closed enum `{water_volume, energy, none}` (§6).
- **OD4 — entity cardinality + external-id encoding:** a single authoritative
  `entity_count` per domain (kills 58/29/31), `index_base: dense_zero_based`, and
  an optional ordered `external_ids` array (`length == entity_count`,
  read-time-cross-checked) for small domains (§5).
- **OD5 — on-disk mapping/attribute encoding:** REUSE the existing bluesmith
  parquet/cog/geoparquet shapes verbatim (the §7 closed `format` set); mappings
  are declared explicitly in `mappings[]` rather than role-string-derived.

## 14. Schema revision allowance

The registry and mapping/domain schemas are FINAL modulo a single, named, logged
escape hatch. A downstream step (A4 / A4b / A5 / A6 / A8) that finds an A2 schema
unrepresentable for the real encoding MAY file ONE schema-change request per
filed instance — a single schema FIELD change — recorded in
`spec/SCHEMA-REVISIONS.md`. Each request re-runs ONLY A2's schema linter +
round-trip example fixtures, NOT a redesign. This is an escape hatch, not a
license to re-litigate the schema mid-stream.
