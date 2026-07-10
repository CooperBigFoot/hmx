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
