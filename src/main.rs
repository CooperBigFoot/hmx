//! The `hmx` binary — thin CLI glue over `hmx-core`.
//!
//! The CLI parses one package path, calls the corresponding `hmx-core` verb, and
//! prints the verb's JSON to stdout. Diagnostics go through `tracing` to stderr.
//! Exit codes are result routing only: `0` for `describe` success or conformant
//! `validate`, `1` for a non-conformant validation report, and `2` for usage or
//! structural errors.

use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing::error;

use hmx_core::describe::describe_json;
use hmx_core::validate::validate;

/// The `hmx` CLI: a thin JSON-emitting surface over the `hmx-core` verbs (A9+).
#[derive(Debug, Parser)]
#[command(name = "hmx", version, about = "Thin JSON-emitting CLI over the hmx-core verbs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The supported subcommands. Each wraps one `hmx-core` verb.
#[derive(Debug, Subcommand)]
enum Command {
    /// Describe an HMX package.
    Describe {
        /// Path to the HMX package root.
        path: PathBuf,
    },
    /// Validate an HMX package.
    Validate {
        /// Path to the HMX package root.
        path: PathBuf,
    },
}

const EXIT_NON_CONFORMANT: u8 = 1;
const EXIT_ERROR: u8 = 2;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    match Cli::parse().command {
        Command::Describe { path } => describe_exit(&path),
        Command::Validate { path } => validate_exit(&path),
    }
}

fn describe_exit(path: &Path) -> ExitCode {
    match describe_json(path) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            error!(path = %path.display(), error = %err, "describe failed");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn validate_exit(path: &Path) -> ExitCode {
    match validate(path) {
        Ok(report) => match report.to_json_string() {
            Ok(json) => {
                println!("{json}");
                if report.conformant() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(EXIT_NON_CONFORMANT)
                }
            }
            Err(err) => {
                error!(path = %path.display(), error = %err, "serializing validation report failed");
                ExitCode::from(EXIT_ERROR)
            }
        },
        Err(err) => {
            error!(path = %path.display(), error = %err, "validate failed");
            ExitCode::from(EXIT_ERROR)
        }
    }
}
