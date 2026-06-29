# HMX — Hydrology Model Exchange

HMX is a prescriptive, friction-anchored interface for single-basin hydrology
**model** packages: a manifest + a typed field registry + cross-domain mappings +
multi-entity domains + a manifest content-hash, exposed through a `validate` /
`describe` CLI and a PyO3 binding. HMX is a **standalone sibling** of HDX/HFX — not
compositional with them, not a superset. One basin per package.

> **Status: skeleton (M13 / step A1).** This repository currently holds only the
> compiling workspace skeleton. The normative spec lands in A2; `hmx-core`
> types/readers in A3+; the CLI verbs in A8/A9; the PyO3 mirror in A10; the
> conformance suite + Stage-A gate in A11.

- Spec: [`spec/HMX_SPEC.md`](spec/HMX_SPEC.md) (authored in A2)
- Core crate: `crates/core` (package `hmx-core`)
- Python binding: `crates/python` (package `hmx-python`)
- CLI: root `Cargo.toml` (binary `hmx`)
