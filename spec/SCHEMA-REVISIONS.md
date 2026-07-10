# HMX schema revision log

The HMX JSON Schemas (`schemas/*.schema.json`) were frozen in step A2. They are
FINAL modulo a single, named, logged escape hatch (spec §14):

- A downstream step (A4 / A4b / A5 / A6 / A8) that finds an A2 schema
  unrepresentable for the real encoding MAY file **one schema-change request per
  filed instance** — a single schema **field** change.
- Each filed request appends a row to the table below and re-runs ONLY A2's
  schema linter (`uvx check-jsonschema --check-metaschema …`) plus the round-trip
  example fixtures under `schemas/examples/`. It is NOT a license to redesign the
  schema mid-stream (the friction log shows F2/F10/F20 were found empirically,
  not by review).

## Log

| Date | Step | Schema | Field changed | Reason | Linter + round-trip re-run |
|------|------|--------|---------------|--------|----------------------------|
| 2026-07-10 | M1-S1 | `describe`, `domain`, `field_registry`, `manifest`, `mapping`, `validate` | Schema `$id` version paths; manifest and describe `format_version` constraints | Deliberate coordinated hard cut: 0.2 is now the only accepted HMX contract version. | Metaschema lint, example/fixture tests, conformance regeneration and blessing, golden schema validation, workspace tests/build, and validator gates. |
| 2026-07-10 | M2-S1 | `field_registry` | `layer_count` | `per_layer` fields had no representable registry-owned cardinality despite §7.3/§11 requiring it. | `uvx check-jsonschema` metaschema lint, positive example validation, scalar-with-layer-count rejection, per-layer-without-layer-count rejection, registry/parser tests, and the scoped core suite. |
| 2026-07-10 | M3-S1 | `derive` | Record schema introduced with content hashes, timestamp, derived name, replacement entries, non-parameter overrides, and tool version. | Establish the external provenance wire contract for `hmx derive` without adding provenance to the manifest. | Draft 2020-12 metaschema lint, `derive.valid.json` validation, and the unchanged workspace suite. |
| 2026-07-10 | M4-S1 | `derive` | `old_sha256` and `new_sha256` are individually nullable, with an at-least-one-real `anyOf` constraint. | Represent materialization's scalar-artifact removal and generated-COG additions without fabricated digests. | Draft 2020-12 metaschema lint, unchanged ordinary derive example, temporary addition/removal/both-null checks, root hmx tests, workspace sanity, build, and golden identity. |
