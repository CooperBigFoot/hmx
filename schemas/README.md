# HMX JSON Schemas

The normative, machine-checkable form of `spec/HMX_SPEC.md` (JSON Schema Draft
2020-12, `additionalProperties:false` throughout). Authored in step A2.

| Schema | Validates |
|---|---|
| `manifest.schema.json` | the package `manifest.json` (spec §3) |
| `domain.schema.json` | a single `domains[]` declaration (spec §5) |
| `mapping.schema.json` | a single `mappings[]` declaration (spec §8) |
| `field_registry.schema.json` | `registry/fields.json` (spec §6) |
| `parameter_scalars.schema.json` | an `hmx/parameter_scalars_v1` scalar-parameter object (spec §7) |
| `describe.schema.json` | the `describe` CLI output (spec §10.2) |
| `validate.schema.json` | the `validate` CLI output (spec §10.3) |
| `derive.schema.json` | the external derivation record shared by `hmx derive` and `hmx materialize` (spec §10.9) |

`examples/` holds tiny hand-authored fixtures used to lint these schemas (valid
fixtures must pass; `*.invalid-*.json` fixtures must be rejected). These are NOT
the conformance suite — the deterministic generator + golden vectors land in step
A11. `derive.valid.json` demonstrates an ordinary `hmx derive` record with
scalar and physical replacements and both digests present in every entry.
`parameter_scalars.valid.json` demonstrates
both a scalar number and a per-layer numeric array;
`parameter_scalars.invalid-value.json` demonstrates a rejected string value.

Lint locally:

    uvx check-jsonschema --check-metaschema schemas/*.schema.json
    uvx check-jsonschema --schemafile schemas/manifest.schema.json schemas/examples/manifest.valid.json
    uvx check-jsonschema --schemafile schemas/derive.schema.json schemas/examples/derive.valid.json
