# HMX JSON Schemas

The normative, machine-checkable form of `spec/HMX_SPEC.md` (JSON Schema Draft
2020-12, `additionalProperties:false` throughout). Authored in step A2.

| Schema | Validates |
|---|---|
| `manifest.schema.json` | the package `manifest.json` (spec §3) |
| `domain.schema.json` | a single `domains[]` declaration (spec §5) |
| `mapping.schema.json` | a single `mappings[]` declaration (spec §8) |
| `field_registry.schema.json` | `registry/fields.json` (spec §6) |
| `describe.schema.json` | the `describe` CLI output (spec §10.2) |
| `validate.schema.json` | the `validate` CLI output (spec §10.3) |

`examples/` holds tiny hand-authored fixtures used to lint these schemas (valid
fixtures must pass; `*.invalid-*.json` fixtures must be rejected). These are NOT
the conformance suite — the deterministic generator + golden vectors land in step
A11.

Lint locally:

    uvx check-jsonschema --check-metaschema schemas/*.schema.json
    uvx check-jsonschema --schemafile schemas/manifest.schema.json schemas/examples/manifest.valid.json
