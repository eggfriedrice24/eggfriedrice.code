//! Runs one parsed command line: gathers the context, dispatches to the command, and
//! turns the outcome into an exit code and, on failure, a message on stderr.

use std::fmt::Write as _;

use crate::cli::{Cli, Command};
use crate::commands::{
    away, compact, config, context, conversation, diff, history, jobs, login, logout, models, new,
    paths, project, resume, sandbox, search, send, settings, shell, status, undo,
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
        Err(error) => match &cli.command {
            Command::Search(args) if args.is_hidden() => {
                search::without_context(args, &error, &mut out)
            }
            _ => report(&error, &mut out, efr_config::DEFAULT_PROVIDER),
        },
    }
}

/// Runs `command` with `ctx`, writing to `out`.
pub(crate) async fn run(command: &Command, ctx: &Context, out: &mut Output) -> Exit {
    // `config show` lists the warnings itself, and the forms of `efr search` that fzf
    // runs write nothing to stderr, which fzf would draw over its rows.
    let quiet = command.is_quiet();
    if !matches!(command, Command::Config(_)) && !quiet {
        for warning in &ctx.settings.warnings {
            out.err(&format!("efr: warning: {}\n", format::one_line(&warning.to_string())));
        }
    }
    let result = match command {
        Command::Send(args) => Box::pin(send::run(ctx, out, args)).await,
        Command::New(args) => Box::pin(new::run(ctx, out, args)).await,
        Command::Status => status::run(ctx, out).await,
        Command::History(args) => history::run(ctx, out, args).await,
        Command::Resume(args) => resume::run(ctx, out, args).await,
        Command::Conversation(command) => conversation::run(ctx, out, command).await,
        Command::Search(args) => search::run(ctx, out, args).await,
        Command::Diff(args) => diff::run(ctx, out, args).await,
        Command::Undo(args) => Box::pin(undo::run(ctx, out, args)).await,
        Command::Compact(args) => compact::run(ctx, out, args).await,
        Command::Context(args) => context::run(ctx, out, args).await,
        Command::Away(args) => away::run(ctx, out, args).await,
        Command::Jobs(args) => Box::pin(jobs::run(ctx, out, args)).await,
        Command::Shell(args) => Box::pin(shell::run(ctx, out, args)).await,
        Command::Settings(args) => settings::run(ctx, out, args).await,
        Command::Models(args) => models::run(ctx, out, args).await,
        Command::Login(command) => login::run(ctx, out, command).await,
        Command::Logout(args) => logout::run(ctx, out, args).await,
        Command::Config(command) => config::run(ctx, out, command).await,
        Command::Paths(args) => paths::run(ctx, out, args).await,
        Command::Project(command) => project::run(ctx, out, command).await,
        Command::Sandbox(command) => sandbox::run(ctx, out, command).await,
    };
    match result {
        Ok(()) => Exit::Success,
        Err(error) if quiet => error.exit(),
        Err(error) => report(&error, out, ctx.settings.provider()),
    }
}

/// Writes the message for `error`, unless it needs none, and returns its exit code. A
/// login hint names the login of `provider`.
fn report(error: &CliError, out: &mut Output, provider: &str) -> Exit {
    if !error.is_silent() {
        out.err(&message_for(error, provider));
    }
    error.exit()
}

/// `efr: ` and the error with its sources on one line, then the hint on a line of its
/// own when there is one, with the default provider in a login hint.
#[cfg(test)]
pub(crate) fn message(error: &CliError) -> String {
    message_for(error, efr_config::DEFAULT_PROVIDER)
}

/// `efr: ` and the error with its sources on one line, then the hint on a line of its
/// own when there is one; a login hint names the login of `provider`.
pub(crate) fn message_for(error: &CliError, provider: &str) -> String {
    let text = efr_stdx::with_causes(error);
    let mut out = format!("efr: {}\n", format::one_line(&text));
    if let Some(hint) = error.hint_for(provider) {
        let _ = writeln!(out, "efr: {hint}");
    }
    out
}

#[cfg(test)]
mod tests;
