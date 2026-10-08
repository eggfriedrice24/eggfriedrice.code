//! Runs one parsed command line: gathers the context, dispatches to the command, and
//! turns the outcome into an exit code and, on failure, a message on stderr.

use std::fmt::Write as _;

use crate::cli::{Cli, Command};
use crate::commands::{
    compact, config, diff, history, login, models, new, paths, project, sandbox, send, settings,
    status,
};
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::format;
use crate::output::Output;
use crate::terminal::TermFacts;

/// The runtime `efr` runs on: one thread. A command waits on one socket and, at most,
/// the key thread, so more workers would only be idle threads in every `,` line; the
/// file work goes to the blocking pool, which starts its threads on demand.
pub(crate) fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()
}

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
    // `config show` lists the warnings itself.
    if !matches!(command, Command::Config(_)) {
        for warning in &ctx.settings.warnings {
            out.err(&format!("efr: warning: {}\n", format::one_line(&warning.to_string())));
        }
    }
    let result = match command {
        Command::Send(args) => send::run(ctx, out, args).await,
        Command::New(args) => new::run(ctx, out, args).await,
        Command::Status => status::run(ctx, out).await,
        Command::History(args) => history::run(ctx, out, args).await,
        Command::Diff(args) => diff::run(ctx, out, args).await,
        Command::Compact(args) => compact::run(ctx, out, args).await,
        Command::Settings(args) => settings::run(ctx, out, args).await,
        Command::Models(args) => models::run(ctx, out, args).await,
        Command::Login(command) => login::run(ctx, out, command).await,
        Command::Config(command) => config::run(ctx, out, command).await,
        Command::Paths(args) => paths::run(ctx, out, args).await,
        Command::Project(command) => project::run(ctx, out, command).await,
        Command::Sandbox(command) => sandbox::run(ctx, out, command).await,
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
    let text = efr_stdx::with_causes(error);
    let mut out = format!("efr: {}\n", format::one_line(&text));
    if let Some(hint) = error.hint() {
        let _ = writeln!(out, "efr: {hint}");
    }
    out
}

#[cfg(test)]
mod tests;
