//! Repository automation that needs Rust: the dependency rule (`deps`), the file rules
//! (`tidy`), the link check of the docs site (`docs-links`), the protocol schema for code
//! generators (`protocol-schema`), the size budget of the
//! release binaries (`bench-size`), the table of two bench runs (`bench-compare`) and,
//! from milestone 1, protocol docs, fixture blessing and projection rebuilds. The justfile composes these;
//! it never reimplements them.

mod bench;
mod config_docs;
mod deps;
mod docs_links;
mod fixtures;
mod output;
mod protocol_docs;
mod protocol_schema;
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
    /// Check the links and anchors of the docs site (docs/ and docs/SUMMARY.md).
    DocsLinks,
    /// Regenerate docs/protocol.md from efr-protocol.
    ProtocolDocs,
    /// Write docs/protocol.schema.json, the protocol for code generators, from
    /// efr-protocol.
    ProtocolSchema {
        /// Fail when the file is not current instead of writing it.
        #[arg(long)]
        check: bool,
    },
    /// Write docs/config.md and docs/config.schema.json from efr-config.
    ConfigDocs {
        /// Fail when either file is not current instead of writing it.
        #[arg(long)]
        check: bool,
    },
    /// Verify the frozen protocol fixtures, or rewrite them with --bless.
    Fixtures {
        /// Rewrite the fixtures instead of verifying them.
        #[arg(long)]
        bless: bool,
    },
    /// Rebuild the SQLite projections from the event log.
    RebuildProjections,
    /// Compare the sizes of the release binaries with xtask/size-budget.json.
    BenchSize {
        /// The directory of the binaries; cargo's target dir `release` by default.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Print the change of each line between two runs of `just bench-perf`.
    BenchCompare {
        /// The directory of the old run's JSON lines.
        old: PathBuf,
        /// The directory of the new run's JSON lines.
        new: PathBuf,
    },
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
        Command::DocsLinks => docs_links::run(&root),
        Command::ProtocolDocs => Ok(protocol_docs::run()),
        Command::ProtocolSchema { check } => protocol_schema::run(&root, check),
        Command::ConfigDocs { check } => config_docs::run(&root, check),
        Command::Fixtures { bless } => Ok(fixtures::run(bless)),
        Command::RebuildProjections => Ok(rebuild_projections::run()),
        Command::BenchSize { dir } => bench::size(&root, dir),
        Command::BenchCompare { old, new } => bench::compare(&old, &new),
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
