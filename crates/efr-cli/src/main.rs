//! `efr`, the command-line relay to the efr daemon. The zsh plugin runs it for every
//! `,` line; people run it for status, history, login, paths and `config.toml`.
//!
//! - `cli`: the command line (clap).
//! - `run`: dispatch, exit codes and error messages; `commands/*`: one file per command.
//! - `follow`: following a turn's events, with `follow/view` deciding what they look
//!   like and `live` redrawing the live zone of a streaming reply.
//! - `keys`: the key thread, for one-key answers to approvals and for answer lines;
//!   `answer`: the line typed for a command that waits for input; `quit`: `Ctrl+\`,
//!   which opens such a line for a silent command; `terminal`: the terminal facts and
//!   size.
//! - `context`: what every command runs with; `settings`: the `[render]` table and the
//!   turn defaults of `config.toml`, and the checks of `efr config`; `turn_settings`:
//!   the mode, model and effort a command asks for; `format`: the CLI's own lines;
//!   `output`: the only writer.
//!
//! Allowed dependencies: `efr-client`, `efr-config`, `efr-render`, `efr-protocol` and
//! `efr-stdx`.
//! What does not belong here: business logic (the daemon decides; this relays), any
//! write to the daemon's database or credentials, and the server side of the protocol
//! (`efr-transport`).
//!
//! This file parses the arguments, sets up tracing to stderr and starts the runtime;
//! everything else happens in `run`.

mod answer;
mod cli;
mod commands;
mod context;
mod error;
mod follow;
mod format;
mod keys;
mod live;
mod output;
mod quit;
mod run;
mod settings;
mod terminal;
#[cfg(test)]
mod testing;
mod turn_settings;

use std::process::ExitCode;

use clap::Parser as _;
use efr_stdx::env::Var;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::context::DEFAULT_LOG_FILTER;
use crate::error::Exit;
use crate::output::Output;
use crate::terminal::TermFacts;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage(&error),
    };
    let term = TermFacts::from_process();
    telemetry(&term);
    let runtime = match run::runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            Output::process().err(&format!("efr: the async runtime could not start: {error}\n"));
            return ExitCode::from(Exit::DaemonError.code());
        }
    };
    let exit = runtime.block_on(run::main(cli, term));
    // NOTE: nothing that matters can still be running here; shutting down in the
    // background means a blocked read of stdin cannot hold up the exit.
    runtime.shutdown_background();
    ExitCode::from(exit.code())
}

/// Prints clap's help, version or usage error and returns its exit code: 0 for help
/// and version, 2 for a usage error.
fn usage(error: &clap::Error) -> ExitCode {
    let text = error.render().to_string();
    let mut out = Output::process();
    if error.use_stderr() {
        out.err(&text);
    } else if out.out(&text).is_err() {
        return ExitCode::from(Exit::DaemonError.code());
    }
    ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(Exit::Usage.code()))
}

/// Logs to stderr, filtered by `EFR_LOG` (warnings and errors by default).
fn telemetry(term: &TermFacts) {
    let wanted = efr_stdx::env::var(Var::Log).ok().flatten();
    let filter = wanted
        .as_deref()
        .and_then(|wanted| EnvFilter::try_new(wanted).ok())
        .or_else(|| EnvFilter::try_new(DEFAULT_LOG_FILTER).ok())
        .unwrap_or_default();
    let installed = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(term.stderr_tty && !term.no_color)
        .with_target(false)
        .compact()
        .try_init();
    // A subscriber can only be missing if one was installed before, which nothing does.
    drop(installed);
}
