# HMX — Build Architecture (PLACEHOLDER)

> **Status: skeleton (M13 / step A1).** A living, build-oriented distillation of
> the HMX spec will live here. The normative contract is authored in
> `spec/HMX_SPEC.md` (step A2); this file will distill it for the build steps
> (A3+). It MUST NOT contradict the spec — on any conflict the spec wins and this
> file is the bug.

## Workspace layout (A1)

- root `Cargo.toml` — the `hmx` CLI binary (clap; `validate` / `describe` verbs).
- `crates/core` — package `hmx-core`: the pure-Rust contract core (types, readers,
  content-hash, verbs). A1 ships a skeleton (version/link proof only).
- `crates/python` — package `hmx-python`: a PyO3 abi3-py312 binding mirroring the
  core verbs. A1 exposes `__core_version` only.
- `spec/` — the normative spec (authored A2). `schemas/` — committed JSON schemas (A2).
- `conformance/` — deterministic fixture suite + goldens (A11).
- `docs/` — ships empty (the docs-for-AI mechanism is out of M13 scope).

## Forward map

A2 spec + schemas → A3 core types/manifest reader → A4/A4b metadata + control-plane
readers → A5 field registry → A6 mappings/domains/cardinality → A7 content-hash →
A8 validate/describe engine → A9 CLI → A10 PyO3 → A11 conformance suite + Stage-A gate.
