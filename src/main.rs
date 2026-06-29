//! The `hmx` binary — thin CLI glue (SKELETON).
//!
//! A1 ships the two-verb command surface (`validate` / `describe`) as STUBS: each
//! parses a package path, logs a "not yet implemented" diagnostic to stderr via
//! `tracing`, and exits 2. The real engine (the `hmx-core` verbs behind these
//! subcommands, the `0 / 1 / 2` exit-code contract, and the stdout-schema
//! conformance) lands in A8 (core) / A9 (CLI). No contract logic lives here yet.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing::error;

/// The `hmx` CLI: a thin JSON-emitting surface over the `hmx-core` verbs (A9+).
#[derive(Debug, Parser)]
#[command(
    name = "hmx",
    version,
    about = "Thin JSON-emitting CLI over the hmx-core verbs (skeleton)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The supported subcommands. Each will wrap one `hmx-core` verb (A9).
#[derive(Debug, Subcommand)]
enum Command {
    /// Describe an HMX package (stub: not yet implemented; lands in A8/A9).
    Describe {
        /// Path to the HMX package root.
        path: PathBuf,
    },
    /// Validate an HMX package (stub: not yet implemented; lands in A8/A9).
    Validate {
        /// Path to the HMX package root.
        path: PathBuf,
    },
}

/// Exit code for a structural / not-yet-implemented condition (the A9 contract
/// reserves `2` for structural/entry errors; the skeleton reuses it).
const EXIT_NOT_IMPLEMENTED: u8 = 2;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let cli = Cli::parse();

    let (verb, path) = match cli.command {
        Command::Describe { path } => ("describe", path),
        Command::Validate { path } => ("validate", path),
    };

    error!(
        verb = verb,
        path = %path.display(),
        core_version = hmx_core::core_version(),
        "hmx subcommand is not yet implemented (lands in A8/A9)"
    );
    ExitCode::from(EXIT_NOT_IMPLEMENTED)
}
