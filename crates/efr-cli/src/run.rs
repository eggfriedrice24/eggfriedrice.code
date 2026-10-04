//! Runs one parsed command line: gathers the context, dispatches to the command, and
//! turns the outcome into an exit code and, on failure, a message on stderr.

use std::error::Error as _;
use std::fmt::Write as _;

use crate::cli::{Cli, Command};
use crate::commands::{history, login, new, send};
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::format;
use crate::output::Output;
use crate::terminal::TermFacts;

/// Runs `cli` in this process.
pub(crate) async fn main(cli: Cli, term: TermFacts) -> Exit {
    let mut out = Output::process();
    match Context::from_process(term).await {
        Ok(ctx) => run(&cli.command, &ctx, &mut out).await,
        Err(error) => report(&error, &mut out),
    }
}

/// Runs `command` with `ctx`, writing to `out`.
pub(crate) async fn run(command: &Command, ctx: &Context, out: &mut Output) -> Exit {
    for warning in &ctx.settings.warnings {
        out.err(&format!("efr: warning: {}\n", format::one_line(&warning.to_string())));
    }
    let result = match command {
        Command::Send(args) => send::run(ctx, out, args).await,
        Command::New(args) => new::run(ctx, out, args).await,
        Command::History(args) => history::run(ctx, out, args).await,
        Command::Login(command) => login::run(ctx, out, command).await,
    };
    match result {
        Ok(()) => Exit::Success,
        Err(error) => report(&error, out),
    }
}

/// Writes the message for `error`, unless it needs none, and returns its exit code.
fn report(error: &CliError, out: &mut Output) -> Exit {
    if !error.is_silent() {
        out.err(&message(error));
    }
    error.exit()
}

/// `efr: ` and the error with its sources on one line, then the hint on a line of its
/// own when there is one.
pub(crate) fn message(error: &CliError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let _ = write!(text, ": {cause}");
        source = cause.source();
    }
    let mut out = format!("efr: {}\n", format::one_line(&text));
    if let Some(hint) = error.hint() {
        let _ = writeln!(out, "efr: {hint}");
    }
    out
}

#[cfg(test)]
mod tests;
