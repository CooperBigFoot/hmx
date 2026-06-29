# Agent Guidelines — hmx

## Project Overview

hmx (Hydrology Model Exchange) — a prescriptive, single-basin hydrology **model**
package interface: a Rust workspace with the `hmx` CLI (root package), a pure-Rust
contract core (`crates/core` = `hmx-core`), and a PyO3 binding (`crates/python` =
`hmx-python`). Standalone sibling of HDX/HFX.

## Version Bumping (mandatory)

**Every commit MUST include a patch version bump.** No exceptions.

Before committing, follow this exact sequence:

1. `./scripts/bump-version.sh patch` — modifies the root `Cargo.toml` `[package]` version.
2. `cargo update -w` (or `cargo build --workspace`) — regenerates `Cargo.lock` so the `hmx` package version in the lock matches `Cargo.toml`.
3. Stage `Cargo.toml` AND `Cargo.lock` alongside the code changes in the same commit.
4. Commit with a conventional commit message.
5. `git tag v$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')` — tag the commit.
6. Confirm `git status --short` is empty after the commit + tag.

**Rules:**
- Patch bumps: automatic with every commit.
- Minor/major bumps: only on explicit request (`./scripts/bump-version.sh minor|major`).
- **Never let tooling create its own commit or tag** — fold the version change into the real commit.
- **Always tag** after every commit. The worktree must be clean afterward.

> `cargo bump` does not support Cargo workspaces (it panics). Use `./scripts/bump-version.sh`.

### Quick Reference

| Command | Effect |
|---|---|
| `./scripts/bump-version.sh patch` | `0.1.0` → `0.1.1` |
| `./scripts/bump-version.sh minor` | `0.1.1` → `0.2.0` |
| `./scripts/bump-version.sh major` | `0.2.0` → `1.0.0` |
| `grep '^version' Cargo.toml` | Show current version |

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
