//! `efr send`: send a prompt to the terminal's conversation and follow the reply, or,
//! with `--steer`, add text to the running turn.
//!
//! The zsh plugin hands the shell context, the last command line and the prompt over
//! in `EFR_CONTEXT`, `EFR_LAST_COMMAND` and `EFR_PROMPT`, not as arguments: any local
//! user can read a command line in `/proc/<pid>/cmdline`, while `/proc/<pid>/environ`
//! is readable only by the user's own processes. The arguments stay for use by hand,
//! and an argument wins over its variable.

use efr_client::Client;
use efr_protocol::{
    ConversationId, ConversationsList, ConversationsListResult, EffectiveSettings, Method, Origin,
    PromptSend, PromptSendResult, ShellContext, TurnSettings, TurnSteer, TurnSteerResult,
};
use efr_render::render_trace;
use efr_stdx::env::Var;

use crate::cli::{LastCommand, SendArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::follow::{self, Compose, Look, Row, Target, TurnView};
use crate::keys::KeyReader;
use crate::live::effective_width;
use crate::output::Output;
use crate::progress;
use crate::row::RowLine;
use crate::turn_settings::Asked;

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
    /// The turn settings that the prompt asks for.
    pub(crate) settings: TurnSettings,
}

/// The parts of a prompt that the zsh plugin hands over, each from its argument or,
/// without one, from its variable.
#[derive(Debug)]
pub(crate) struct Handover {
    pub(crate) context: ShellContext,
    /// `shell` when the context came from the plugin, `cli` when it is the process's.
    pub(crate) origin: Origin,
    pub(crate) last_command: Option<LastCommand>,
    pub(crate) text: String,
}

impl Handover {
    /// Reads the handover for a command with these arguments.
    pub(crate) fn read(
        ctx: &Context,
        context_json: Option<&str>,
        last_command: Option<&LastCommand>,
        prompt: &[String],
    ) -> Result<Handover, CliError> {
        let var = |var| ctx.env.var(var).map_err(|source| CliError::Environment { source });
        let context = match context_json {
            Some(json) => Some(("--context-json", json.to_owned())),
            None => var(Var::Context)?.map(|json| (Var::Context.name(), json)),
        };
        let origin = origin_of(context.is_some());
        let context = shell_context(ctx, context)?;
        let last_command = match last_command {
            Some(line) => Some(line.clone()),
            None => var(Var::LastCommand)?.map(LastCommand::from),
        };
        let text = if prompt.is_empty() {
            var(Var::Prompt)?.unwrap_or_default()
        } else {
            prompt_text(prompt)
        };
        Ok(Handover { context, origin, last_command, text })
    }
}

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &SendArgs) -> Result<(), CliError> {
    let handover = Handover::read(
        ctx,
        args.context_json.as_deref(),
        args.last_command.as_ref(),
        &args.prompt,
    )?;
    let Handover { context, origin, last_command, text } = handover;
    if text.trim().is_empty() {
        return Err(CliError::EmptyPrompt);
    }
    if args.steer {
        // A steer joins a turn whose context is set, so a last command has no place.
        return steer(ctx, out, args.conversation, &context, origin, text).await;
    }
    let prompt = Prompt {
        conversation: args.conversation,
        new_conversation: false,
        text,
        context,
        last_command,
        settings: Asked::read(ctx, &args.settings)?.to_wire(),
    };
    send(ctx, out, origin, prompt).await
}

/// Sends `prompt` and follows the turn that answers it. On a terminal with
/// `render.turn_input` on, the input row's key reader starts first, so the keys typed
/// while the prompt goes out land in the row; a failure before the turn is followed
/// hands what they typed back to the shell.
pub(crate) async fn send(
    ctx: &Context,
    out: &mut Output,
    origin: Origin,
    prompt: Prompt,
) -> Result<(), CliError> {
    let mut reader = if row_wanted(ctx) {
        ctx.keys
            .keep()
            .inspect_err(|error| tracing::debug!(%error, "the input row cannot read keys"))
            .ok()
    } else {
        None
    };
    let result = send_and_follow(ctx, out, origin, prompt, &mut reader).await;
    if let Some(reader) = reader {
        give_back(ctx, out, reader).await;
    }
    result
}

/// True when a followed turn gets the input row: `render.turn_input` is on, and stdin
/// and stdout are terminals.
fn row_wanted(ctx: &Context) -> bool {
    ctx.settings.render.turn_input && ctx.keys.available() && ctx.term.formats_stdout()
}

/// Stops `reader`, which no turn took over, and hands the keys that it read back to the
/// shell.
async fn give_back(ctx: &Context, out: &mut Output, reader: KeyReader) {
    let mut line = RowLine::default();
    for key in reader.stop_keeping().await {
        line.key(key);
    }
    if let Some(note) = crate::draft::hand_back(ctx, line.text()).await {
        let options = ctx.render_options(effective_width(ctx.screen.size()));
        out.err(&render_trace(&note, &options));
    }
}

/// Sends `prompt` and follows the turn, with the input row when `reader` holds its key
/// reader; the follow loop takes the reader.
async fn send_and_follow(
    ctx: &Context,
    out: &mut Output,
    origin: Origin,
    prompt: Prompt,
    reader: &mut Option<KeyReader>,
) -> Result<(), CliError> {
    let compose = Compose {
        context: prompt.context.clone(),
        last_command: prompt.last_command.clone(),
        settings: prompt.settings.clone(),
    };
    let client = ctx.connect(origin, prompt.context.tty.as_deref()).await?;
    let result = send_prompt(ctx, &client, prompt).await?;
    let size = ctx.screen.size();
    let options = ctx.render_options(effective_width(size));
    let render = &ctx.settings.render;
    let look = Look {
        motion: render.motion,
        summary: render.turn_summary,
        progress: progress::wanted(render.progress, &ctx.term),
        diff_lines: usize::try_from(render.diff_lines).unwrap_or(usize::MAX),
    };
    let mut view =
        TurnView::new(result.turn_id, options).with_home(ctx.home.clone()).with_look(look);
    if reader.is_some() {
        view = view.with_input();
    }
    // NOTE: the note comes from the prompt.send result, so it is the reply's first line
    // even before the first event arrives; the follow loop writes it in its first frame.
    if let Some(note) = result.settings.as_ref().and_then(overrides) {
        let step = view.note(&note, size);
        out.err(&step.err);
        out.out(&step.out)?;
    }
    if result.queued {
        view.queue();
        let step = view.note("queued behind the running turn", size);
        out.err(&step.err);
        out.out(&step.out)?;
    }
    view.start();
    let target =
        Target { conversation: result.conversation_id, turn: result.turn_id, after: result.seq };
    let row = reader.take().map(|reader| Row { compose, reader });
    follow::follow(ctx, &client, out, &mut view, target, row).await
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
        settings: prompt.settings,
    });
    Ok(client.call(method).await?)
}

/// The values of `settings` that the prompt set, such as `mode auto, model gpt-5.4`;
/// `None` when the config gave every one.
pub(crate) fn overrides(settings: &EffectiveSettings) -> Option<String> {
    crate::format::turn_settings(settings, false)
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
        if_late: None,
    });
    let _: TurnSteerResult = client.call(method).await?;
    let size = ctx.screen.size();
    let options = ctx.render_options(effective_width(size));
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

/// The shell context to send: the plugin's, given as JSON with the name of the
/// argument or variable it came from, or, typed by hand, the working directory and the
/// terminal on stdin.
fn shell_context(
    ctx: &Context,
    json: Option<(&'static str, String)>,
) -> Result<ShellContext, CliError> {
    match json {
        Some((input, json)) => {
            serde_json::from_str(&json).map_err(|source| CliError::InvalidContext { input, source })
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
fn origin_of(from_plugin: bool) -> Origin {
    if from_plugin { Origin::Shell } else { Origin::Cli }
}

/// The prompt words joined with single spaces, as the shell split them.
fn prompt_text(words: &[String]) -> String {
    words.join(" ")
}

#[cfg(test)]
mod tests;
