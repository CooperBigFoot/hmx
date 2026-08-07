# Agent Guidelines — hmx

## Project Overview

hmx (Hydrology Model Exchange) — a prescriptive, single-basin hydrology **model**
package interface: a Rust workspace with the `hmx` CLI (root package), a pure-Rust
contract core (`crates/core` = `hmx-core`), and a PyO3 binding (`crates/python` =
`hmx-python`). Standalone sibling of HDX/HFX.

## Formatting, Linting & Testing (changed-lines-only discipline)

- **Never run `cargo fmt` (or any whole-file / whole-repo formatter) across files you did not author.** Reformatting code you did not write produces unrelated diff churn (friction F13.6 / F18 / F24). Format only the lines you authored.
- **Never gate on a repo-wide `cargo fmt --check` or `cargo clippy --workspace -- -D warnings`.** These are false-fail gates on a multi-author tree (F13). Scope clippy to the crates you changed; treat warnings as advisory, not a hard block.
- **Scoped tests during implementation** (`cargo test -p <crate>`); the full `cargo test --workspace` sweep runs only at stage / milestone close (F23).

## Rust Coding Conventions

### Logging: `tracing`, not `log`

Use the `tracing` crate exclusively. Never use `println!` or the `log` crate for diagnostics (`println!` only for an actual JSON output value).

- Use structured fields (`key = value`) over format strings.
- Use `#[instrument]` on public functions; `skip` large args.
- Levels: `error` = broken, `warn` = degraded, `info` = milestones, `debug` = internals, `trace` = hot loops.

### Error Handling

- **Library code** (`crates/`): use `thiserror`. Every variant gets a doc comment explaining _when_ it fires. Named fields, not tuples.
- **Application code** (`src/`): use `anyhow` with `.context()`.
- **Never `.unwrap()` / `.expect()` in library code.** In `main.rs` / CLI glue, `.expect("reason")` is acceptable only for truly unrecoverable situations.

### Documentation — LLM-Agent-First, Intentional

- Simple module (<~150 lines): a one-line `//!` purpose comment.
- Complex crate (multiple files, non-obvious interactions): a crate-root `README.md` (purpose, Mermaid architecture diagram, glossary, key types).
- Function/type docs: first line = single imperative sentence; add detail only when the code isn't self-evident; `# Errors` for fallible public fns; `# Panics` if debug-asserts exist; `[backtick links]` to cross-reference.
- Skip doc comments on obvious helpers, private internals, trivial getters.
- **Diagrams: Mermaid, never ASCII art**, and in crate READMEs (not inline) to keep `.rs` files lean.

### Type-Driven Development (strict)

- **Parse, don't validate (hard rule):** raw input is converted into typed domain representations at the system boundary; internal functions never accept raw primitives when a domain type exists.
- **Newtype wrappers:** wrap where confusion between semantically different quantities is plausible (coords, IDs, thresholds, indices). Bare primitives OK for unambiguous locals.
- **Enums over booleans (always):** never use `bool` for a domain state with two named possibilities.
- **Typestate pattern:** use for pipelines / multi-step workflows / lifecycles; don't force it everywhere.

### Code Style

- Prefer iterators over indexed loops.
- Derive liberally: `#[derive(Debug, Clone, PartialEq)]` on public types unless there's a reason not to.
- Builder pattern for config structs with >3 fields.
- Keep struct fields private; public fields only for plain-data/config types.
- Math-friendly names allowed in algorithm code, with a module-doc glossary.
- **No `use super::*`** — explicit imports only.
- **Group imports**: std → external crates → crate-internal, separated by blank lines.

<!-- BEGIN SYNCED DOCTRINE; source-sha256=59e37fd6b3dbab27530822e6956da51bb7ae76b637e3638530f99a8b4db9038d -->
Four rules. They are one design stance seen four ways: a module means one thing, receives exactly what it needs, in types that cannot lie, and dies rather than guess.

1. **A module means one thing.**
2. **It receives exactly what it needs.**
3. **Its types cannot lie.**
4. **It dies rather than guess.**
<!-- END SYNCED DOCTRINE -->
