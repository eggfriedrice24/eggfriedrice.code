//! `efr new`: start a new conversation for the terminal, and follow the reply when a
//! first prompt comes with it.
//!
//! Without a prompt, `prompt.send` goes out with `new_conversation` and empty text, and
//! nothing is followed: the daemon makes the new conversation the terminal's active
//! one, so the next `,` line continues it.

use efr_protocol::Origin;
use efr_render::render_trace;

use crate::cli::NewArgs;
use crate::commands::send::{self, Prompt};
use crate::context::Context;
use crate::error::CliError;
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &NewArgs) -> Result<(), CliError> {
    let context = send::shell_context(ctx, args.context_json.as_deref())?;
    let origin = send::origin_of(args.context_json.is_some());
    let text = send::prompt_text(&args.prompt);
    let prompt = Prompt {
        conversation: None,
        new_conversation: true,
        text,
        context,
        last_command: args.last_command.clone(),
    };
    if prompt.text.trim().is_empty() {
        start_only(ctx, out, origin, prompt).await
    } else {
        send::send(ctx, out, origin, prompt).await
    }
}

/// Starts the conversation without a prompt to answer.
async fn start_only(
    ctx: &Context,
    out: &mut Output,
    origin: Origin,
    prompt: Prompt,
) -> Result<(), CliError> {
    let client = ctx.connect(origin, prompt.context.tty.as_deref()).await?;
    let result = send::send_prompt(ctx, &client, Prompt { text: String::new(), ..prompt }).await?;
    let size = ctx.screen.size();
    let options = ctx.term.render_options(effective_width(size), ctx.settings.theme);
    let line = render_trace(&format!("new conversation {}", result.conversation_id), &options);
    if options.is_terminal() {
        out.out(&line)
    } else {
        out.err(&line);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
