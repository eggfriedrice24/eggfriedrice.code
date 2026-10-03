//! Repository automation that needs Rust: the dependency rule (`deps`), the file rules
//! (`tidy`) and, from milestone 1, protocol docs, fixture blessing and projection
//! rebuilds. The justfile composes these; it never reimplements them.

mod deps;
mod fixtures;
mod output;
mod protocol_docs;
mod rebuild_projections;
mod tidy;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Repository checks and generators for eggfriedrice.code")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check workspace dependency edges against the allowlist and the forbidden edges.
    Deps {
        /// Print the result as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Check repository files against the tidy rules in CONVENTIONS.md.
    Tidy {
        /// Run only the cheap per-file rules used by the pre-commit hook.
        #[arg(long)]
        fast: bool,
    },
    /// Regenerate docs/protocol.md from efr-protocol.
    ProtocolDocs,
    /// Verify the frozen protocol fixtures, or rewrite them with --bless.
    Fixtures {
        /// Rewrite the fixtures instead of verifying them.
        #[arg(long)]
        bless: bool,
    },
    /// Rebuild the SQLite projections from the event log.
    RebuildProjections,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            output::error(&format!("{err:#}"));
            ExitCode::FAILURE
        }
    }
}

/// Returns `Ok(false)` when a check ran and found violations, so the exit code
/// distinguishes "the repository is wrong" from "the check could not run" only in
/// the printed message, never in a panic.
fn run(command: Command) -> anyhow::Result<bool> {
    let root = repo_root()?;
    match command {
        Command::Deps { json } => deps::run(&root, json),
        Command::Tidy { fast } => tidy::run(&root, fast),
        Command::ProtocolDocs => Ok(protocol_docs::run()),
        Command::Fixtures { bless } => Ok(fixtures::run(bless)),
        Command::RebuildProjections => Ok(rebuild_projections::run()),
    }
}

/// The xtask always lives at `<root>/xtask`, so the root is known at compile time and
/// the commands work from any current directory.
fn repo_root() -> anyhow::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .context("the xtask manifest directory has no parent")
}
