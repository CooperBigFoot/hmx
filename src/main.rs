//! The `hmx` binary — thin CLI glue over `hmx-core`.
//!
//! The CLI parses one package path, calls the corresponding `hmx-core` verb, and
//! prints the verb's JSON to stdout. Diagnostics go through `tracing` to stderr.
//! Exit codes are result routing only: `0` for `describe` success or conformant
//! `validate`, `1` for a non-conformant validation report, and `2` for usage or
//! structural errors.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand};
use tracing::error;

use hmx_core::describe::describe_json;
use hmx_core::validate::validate;

mod derive;

/// The `hmx` CLI: a thin JSON-emitting surface over the `hmx-core` verbs (A9+).
#[derive(Debug, Parser)]
#[command(
    name = "hmx",
    version,
    about = "Thin JSON-emitting CLI over the hmx-core verbs"
)]
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
    /// Derive a standalone HMX package.
    Derive {
        /// Base HMX package root.
        #[arg(value_name = "BASE")]
        base: PathBuf,
        /// New standalone HMX package root.
        #[arg(value_name = "OUT")]
        out: PathBuf,
        /// Name for the derived package.
        #[arg(long, value_name = "NAME", required = true)]
        name: String,
        /// Replace the artifact supplying an exact FieldId.
        #[arg(long, value_name = "FIELDID=FILE", action = ArgAction::Append)]
        replace: Vec<String>,
        /// Set an exact scalar FieldId to one JSON value.
        #[arg(long, value_name = "FIELDID=JSON", action = ArgAction::Append)]
        set: Vec<String>,
        /// Allow replacements to affect non-parameter fields.
        #[arg(long)]
        allow_non_parameter: bool,
        /// Write an identical external derivation record.
        #[arg(long, value_name = "PATH")]
        record: Option<PathBuf>,
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
        Command::Derive {
            base,
            out,
            name,
            replace,
            set,
            allow_non_parameter,
            record,
        } => derive_exit(base, out, name, replace, set, allow_non_parameter, record),
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_exit(
    base: PathBuf,
    out: PathBuf,
    name: String,
    replace: Vec<String>,
    set: Vec<String>,
    allow_non_parameter: bool,
    record: Option<PathBuf>,
) -> ExitCode {
    let operation = || -> Result<Vec<u8>> {
        let request = derive::DeriveRequest::new(
            base.clone(),
            out.clone(),
            name,
            replace,
            set,
            allow_non_parameter,
            record,
        )
        .context("parsing derive request")?;
        derive::execute(request).with_context(|| {
            format!(
                "deriving standalone package from {} to {}",
                base.display(),
                out.display()
            )
        })
    };
    match operation() {
        Ok(bytes) => match std::io::stdout().write_all(&bytes) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                error!(error = %err, "writing derivation record to stdout failed");
                ExitCode::from(EXIT_ERROR)
            }
        },
        Err(err) => {
            error!(base = %base.display(), out = %out.display(), error = %format!("{err:#}"), "derive failed");
            ExitCode::from(EXIT_ERROR)
        }
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
