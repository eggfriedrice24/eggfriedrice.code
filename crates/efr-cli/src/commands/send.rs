//! `efr send`: send a prompt to the terminal's conversation and follow the reply, or,
//! with `--steer`, add text to the running turn.

use efr_client::Client;
use efr_protocol::{
    ConversationId, ConversationsList, ConversationsListResult, Method, Origin, PromptSend,
    PromptSendResult, ShellContext, TurnSteer, TurnSteerResult,
};
use efr_render::render_trace;

use crate::cli::{LastCommand, SendArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::follow::{self, Target, TurnView};
use crate::live::effective_width;
use crate::output::Output;

/// Conversations asked for per page while looking for the terminal's active one.
const PAGE: u32 = 100;
/// Pages read at most while looking for it; the active one is recently updated.
const MAX_PAGES: usize = 10;

/// A prompt ready to send.
#[derive(Debug)]
pub(crate) struct Prompt {
    pub(crate) conversation: Option<ConversationId>,
    pub(crate) new_conversation: bool,
    pub(crate) text: String,
    pub(crate) context: ShellContext,
    pub(crate) last_command: Option<LastCommand>,
}

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &SendArgs) -> Result<(), CliError> {
    let context = shell_context(ctx, args.context_json.as_deref())?;
    let origin = origin_of(args.context_json.is_some());
    let text = prompt_text(&args.prompt);
    if text.trim().is_empty() {
        return Err(CliError::EmptyPrompt);
    }
    if args.steer {
        return steer(ctx, out, args.conversation, &context, origin, text).await;
    }
    let prompt = Prompt {
        conversation: args.conversation,
        new_conversation: false,
        text,
        context,
        last_command: args.last_command.clone(),
    };
    send(ctx, out, origin, prompt).await
}

/// Sends `prompt` and follows the turn that answers it.
pub(crate) async fn send(
    ctx: &Context,
    out: &mut Output,
    origin: Origin,
    prompt: Prompt,
) -> Result<(), CliError> {
    let client = ctx.connect(origin, prompt.context.tty.as_deref()).await?;
    let result = send_prompt(ctx, &client, prompt).await?;
    let size = ctx.screen.size();
    let options = ctx.term.render_options(effective_width(size), ctx.settings.theme);
    let mut view = TurnView::new(result.turn_id, options);
    if result.queued {
        let step = view.note("queued behind the running turn", size);
        out.err(&step.err);
        out.out(&step.out)?;
    }
    let target =
        Target { conversation: result.conversation_id, turn: result.turn_id, after: result.seq };
    follow::follow(ctx, &client, out, &mut view, target).await
}

/// Sends `prompt` without following the turn.
pub(crate) async fn send_prompt(
    ctx: &Context,
    client: &Client,
    prompt: Prompt,
) -> Result<PromptSendResult, CliError> {
    let method = Method::PromptSend(PromptSend {
        command_id: ctx.command_id(),
        conversation_id: prompt.conversation,
        new_conversation: prompt.new_conversation,
        text: prompt.text,
        context: Some(prompt.context),
        last_command: prompt.last_command.map(LastCommand::into_string),
    });
    Ok(client.call(method).await?)
}

/// Adds `text` to the running turn of the conversation, or of the terminal's active
/// one.
async fn steer(
    ctx: &Context,
    out: &mut Output,
    conversation: Option<ConversationId>,
    context: &ShellContext,
    origin: Origin,
    text: String,
) -> Result<(), CliError> {
    let tty = context.tty.as_deref();
    if conversation.is_none() && tty.is_none() {
        return Err(CliError::SteerNeedsConversation);
    }
    let client = ctx.connect(origin, tty).await?;
    let conversation_id = match (conversation, tty) {
        (Some(id), _) => id,
        (None, Some(tty)) => active_conversation(&client, tty).await?,
        (None, None) => return Err(CliError::SteerNeedsConversation),
    };
    let method = Method::TurnSteer(TurnSteer {
        command_id: ctx.command_id(),
        conversation_id,
        turn_id: None,
        text,
    });
    let _: TurnSteerResult = client.call(method).await?;
    let size = ctx.screen.size();
    let options = ctx.term.render_options(effective_width(size), ctx.settings.theme);
    let line = render_trace("steered the running turn", &options);
    if options.is_terminal() {
        out.out(&line)
    } else {
        out.err(&line);
        Ok(())
    }
}

/// The conversation that is active in `tty`.
pub(crate) async fn active_conversation(
    client: &Client,
    tty: &str,
) -> Result<ConversationId, CliError> {
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let method = Method::ConversationsList(ConversationsList {
            cursor: cursor.take(),
            limit: Some(PAGE),
        });
        let page: ConversationsListResult = client.call(method).await?;
        if let Some(found) =
            page.conversations.iter().find(|summary| summary.tty.as_deref() == Some(tty))
        {
            return Ok(found.id);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Err(CliError::NoActiveConversation { tty: tty.to_owned() })
}

/// The shell context to send: the plugin's, or, typed by hand, the working directory
/// and the terminal on stdin.
pub(crate) fn shell_context(ctx: &Context, json: Option<&str>) -> Result<ShellContext, CliError> {
    match json {
        Some(json) => {
            serde_json::from_str(json).map_err(|source| CliError::InvalidContext { source })
        }
        None => {
            let mut context = ShellContext::new(ctx.cwd.clone().unwrap_or_default());
            context.tty.clone_from(&ctx.tty);
            Ok(context)
        }
    }
}

/// The origin a connection announces: the shell when the plugin sent its context,
/// otherwise `efr` used by hand or from a script.
pub(crate) fn origin_of(from_plugin: bool) -> Origin {
    if from_plugin { Origin::Shell } else { Origin::Cli }
}

/// The prompt words joined with single spaces, as the shell split them.
pub(crate) fn prompt_text(words: &[String]) -> String {
    words.join(" ")
}

#[cfg(test)]
mod tests;
