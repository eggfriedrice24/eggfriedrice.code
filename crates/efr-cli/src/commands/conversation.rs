//! `efr conversation delete <id>` (`conversation.delete`): delete a conversation and
//! everything that efr keeps of it. The `,resume` picker deletes with the same steps
//! (Ctrl+D, `resume.rs`).
//!
//! The id is a whole id or the start of exactly one, as for `efr resume`. On a
//! terminal efr asks first, on one line (`y` deletes, anything else or Esc does not);
//! `--yes` deletes with no question, and without a terminal efr deletes only with
//! `--yes`. When another open terminal holds the conversation, or a turn runs in it,
//! efrd deletes nothing: on a terminal efr asks once more and sends the delete again
//! with `force` or `stop`; in a pipe it fails with the hint `--force`, which sends both
//! from the start. The connection's hello names the terminal of the context, so the
//! terminal that asks never counts as one that holds the conversation.

use efr_protocol::{
    ConversationDelete, ConversationDeleteResult, ConversationId, ErrorBody, ErrorCode, Method,
};
use efr_render::RenderOptions;
use efr_transport::client::{Client, ClientError};

use crate::cli::{ConversationCommand, ConversationDeleteArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, delete};
use crate::live::effective_width;
use crate::output::Output;

use super::resume::{self, call, yes};
use super::send::Handover;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &ConversationCommand,
) -> Result<(), CliError> {
    match command {
        ConversationCommand::Delete(args) => delete_command(ctx, out, args).await,
    }
}

async fn delete_command(
    ctx: &Context,
    out: &mut Output,
    args: &ConversationDeleteArgs,
) -> Result<(), CliError> {
    if !args.yes && !ctx.keys.available() {
        return Err(CliError::DeleteNeedsYes);
    }
    let Handover { context, origin, .. } = Handover::read(ctx, None, None, &[])?;
    let client = ctx.connect(origin, context.tty.as_deref()).await?;
    let (id, title) = match resume::find(&client, &args.id).await {
        Ok(summary) => (summary.id, summary.title),
        // NOTE: a whole id that the list does not reach still names a conversation;
        // efrd says when it does not exist.
        Err(CliError::ConversationNotFound { .. }) if args.id.parse::<ConversationId>().is_ok() => {
            (resume::resolve(&client, &args.id).await?, None)
        }
        Err(error) => return Err(error),
    };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let asked = Asked { title: title.as_deref(), short_id: &short_id(id), target: &args.id };
    if !args.yes && !ask(ctx, out, &delete::question(asked.title, asked.short_id), &options).await?
    {
        nothing(out, &options);
        return Ok(());
    }
    delete_loop(ctx, out, &client, id, &asked, args.force).await.map(|_| ())
}

/// What the questions and lines of a delete name: the title, the short id, and what
/// the user typed, for the hint of a pipe.
pub(crate) struct Asked<'a> {
    pub(crate) title: Option<&'a str>,
    pub(crate) short_id: &'a str,
    pub(crate) target: &'a str,
}

/// Sends the delete of `id` and asks on a terminal when efrd answers that another
/// terminal holds it or that a turn runs in it; true when the conversation is deleted.
/// `force` sends `force` and `stop` from the start.
pub(crate) async fn delete_loop(
    ctx: &Context,
    out: &mut Output,
    client: &Client,
    id: ConversationId,
    asked: &Asked<'_>,
    force: bool,
) -> Result<bool, CliError> {
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let (mut held_ok, mut stop) = (force, force);
    loop {
        let params = ConversationDelete {
            command_id: ctx.command_id(),
            conversation_id: id,
            force: held_ok,
            stop,
        };
        match call::<ConversationDeleteResult>(client, Method::ConversationDelete(params)).await {
            Ok(ConversationDeleteResult::Deleted { title, jobs_stopped, not_removed, .. }) => {
                let title = title.as_deref().or(asked.title);
                let line = delete::deleted_line(title, asked.short_id, jobs_stopped);
                out.out(&format!("{}\n", format::paint(&line, Tone::Dim, &options)))?;
                for path in &not_removed {
                    let line = delete::not_removed_line(path);
                    out.err(&format!("{}\n", format::paint(&line, Tone::Attention, &options)));
                }
                return Ok(true);
            }
            Ok(ConversationDeleteResult::Held { tty, title }) if !held_ok => {
                let title = title.as_deref().or(asked.title);
                if !ctx.keys.available() {
                    return Err(CliError::DeleteHeld {
                        title: title.map(str::to_owned),
                        tty,
                        target: asked.target.to_owned(),
                    });
                }
                let question = delete::held_question(title, asked.short_id, &tty);
                if !ask(ctx, out, &question, &options).await? {
                    nothing(out, &options);
                    return Ok(false);
                }
                held_ok = true;
            }
            Err(CliError::Client(ClientError::Server { body }))
                if body.code == ErrorCode::Busy && !stop && turn_runs(&body) =>
            {
                if !ctx.keys.available() {
                    return Err(CliError::DeleteBusy { target: asked.target.to_owned() });
                }
                if !ask(ctx, out, &delete::busy_question(asked.title, asked.short_id), &options)
                    .await?
                {
                    nothing(out, &options);
                    return Ok(false);
                }
                stop = true;
            }
            Ok(_) => {
                let line =
                    "efrd answered in a way that this efr does not know; nothing changed here";
                out.err(&format!("{}\n", format::paint(line, Tone::Dim, &options)));
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
    }
}

/// True for the refusal of a conversation with a running turn, which a stop can lift;
/// not for an undo that runs, or a store that was busy for a moment.
fn turn_runs(body: &ErrorBody) -> bool {
    body.data.as_ref().and_then(|data| data.get("busy")).is_some_and(|busy| busy == "turn")
}

/// Prints `question` and reads the answer; true for yes.
async fn ask(
    ctx: &Context,
    out: &mut Output,
    question: &str,
    options: &RenderOptions,
) -> Result<bool, CliError> {
    out.out(&format!("{}\n", format::paint(question, Tone::Attention, options)))?;
    yes(ctx).await
}

fn nothing(out: &mut Output, options: &RenderOptions) {
    out.err(&format!("{}\n", format::paint("nothing was deleted", Tone::Dim, options)));
}

/// The first 8 characters of `id`, as a row of a search shows them.
pub(crate) fn short_id(id: ConversationId) -> String {
    id.to_string().chars().take(8).collect()
}

#[cfg(test)]
mod tests;
