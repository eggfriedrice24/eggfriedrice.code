//! `efr send`: send a prompt to the terminal's conversation and follow the reply, or,
//! with `--steer`, add text to the running turn.
//!
//! The zsh plugin hands the shell context, the last command line, the recent command
//! lines and the prompt over in `EFR_CONTEXT`, `EFR_LAST_COMMAND`,
//! `EFR_RECENT_COMMANDS` and `EFR_PROMPT`, not as arguments: any local user can read a
//! command line in `/proc/<pid>/cmdline`, while `/proc/<pid>/environ` is readable only
//! by the user's own processes. The arguments stay for use by hand, and an argument
//! wins over its variable. The recent commands have no argument.
//!
//! For a paste whose lines are all `,` prompts, the plugin hands the first one over in
//! `EFR_PROMPT` and the others in `EFR_QUEUED_PROMPTS`, one per line. They go behind
//! the first one, in their order: with the input row, efrd queues them at once, as Tab
//! in the row does, and the view follows each one; without the row, each one goes when
//! the turn before it ended. Those that did not go go back to the shell as `,` lines.

mod recent;
mod shell;

use std::collections::VecDeque;

use efr_protocol::{
    ConversationId, ConversationsList, ConversationsListResult, EffectiveSettings, Method, Origin,
    PromptSend, PromptSendResult, RecentCommand, ShellContext, TurnSettings, TurnSteer,
    TurnSteerResult,
};
use efr_render::render_trace;
use efr_stdx::env::Var;
use efr_transport::client::{Client, ClientError};

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
    /// The terminal's recent commands, oldest first.
    pub(crate) recent_commands: Vec<RecentCommand>,
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
    /// From the variable only: no argument takes them.
    pub(crate) recent_commands: Vec<RecentCommand>,
    pub(crate) text: String,
    /// The prompts to queue behind `text`, in their order; from the variable only.
    pub(crate) queued: Vec<String>,
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
        let recent_commands =
            var(Var::RecentCommands)?.map(|json| recent::parse(&json)).unwrap_or_default();
        let text = if prompt.is_empty() {
            var(Var::Prompt)?.unwrap_or_default()
        } else {
            prompt_text(prompt)
        };
        let queued = var(Var::QueuedPrompts)?.map(|lines| queued(&lines)).unwrap_or_default();
        Ok(Handover { context, origin, last_command, recent_commands, text, queued })
    }
}

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &SendArgs) -> Result<(), CliError> {
    let handover = Handover::read(
        ctx,
        args.context_json.as_deref(),
        args.last_command.as_ref(),
        &args.prompt,
    )?;
    let Handover { context, origin, last_command, recent_commands, text, queued } = handover;
    if text.trim().is_empty() {
        return Err(CliError::EmptyPrompt);
    }
    if args.steer {
        // A steer joins a turn whose context is set, so a last command, the recent
        // commands and prompts to queue have no place.
        return steer(ctx, out, args.conversation, &context, origin, text).await;
    }
    let prompt = Prompt {
        conversation: args.conversation,
        new_conversation: false,
        text,
        context,
        last_command,
        recent_commands,
        settings: Asked::read(ctx, &args.settings)?.to_wire(),
    };
    send(ctx, out, origin, prompt, queued).await
}

/// Sends `prompt` and follows the turn that answers it, then the prompts of `queued`
/// in their order. On a terminal with `render.turn_input` on, the input row's key
/// reader starts first, so the keys typed while the prompt goes out land in the row; a
/// failure before the turn is followed hands what they typed back to the shell, with
/// the queued prompts that did not go.
pub(crate) async fn send(
    ctx: &Context,
    out: &mut Output,
    origin: Origin,
    prompt: Prompt,
    queued: Vec<String>,
) -> Result<(), CliError> {
    let mut reader = if row_wanted(ctx) {
        ctx.keys
            .keep()
            .inspect_err(|error| tracing::debug!(%error, "the input row cannot read keys"))
            .ok()
    } else {
        None
    };
    let mut queued = VecDeque::from(queued);
    let compose = compose(&prompt);
    let mut result = send_and_follow(ctx, out, origin, prompt, &mut reader, &mut queued).await;
    // NOTE: with the input row, efrd took the queued prompts already. Without it, each
    // one goes when the turn before it ended, so its answer shows here too.
    while let Ok(conversation) = result {
        let Some(text) = queued.pop_front() else { break };
        let next = Prompt {
            conversation: Some(conversation),
            new_conversation: false,
            text,
            context: compose.context.clone(),
            last_command: compose.last_command.clone(),
            recent_commands: compose.recent_commands.clone(),
            settings: compose.settings.clone(),
        };
        result = send_and_follow(ctx, out, origin, next, &mut None, &mut VecDeque::new()).await;
    }
    let unsent = unsent(&queued);
    if let Some(reader) = reader {
        give_back(ctx, out, reader, &unsent).await;
    } else if !unsent.is_empty() {
        hand_back(ctx, out, &unsent).await;
    }
    result.map(|_| ())
}

/// What a queued prompt sends with: the same as the prompt before it.
fn compose(prompt: &Prompt) -> Compose {
    Compose {
        context: prompt.context.clone(),
        last_command: prompt.last_command.clone(),
        recent_commands: prompt.recent_commands.clone(),
        settings: prompt.settings.clone(),
    }
}

/// The prompts of `EFR_QUEUED_PROMPTS`: one per line, without the blank lines.
fn queued(lines: &str) -> Vec<String> {
    lines.lines().filter(|line| !line.trim().is_empty()).map(str::to_owned).collect()
}

/// The queued prompts that did not go, as the text that goes back to the shell: the
/// plugin puts `, ` before it, and each later prompt starts its own line with `, `, so
/// Enter sends them again as a paste of `,` lines does.
fn unsent(queued: &VecDeque<String>) -> String {
    queued.iter().map(String::as_str).collect::<Vec<_>>().join("\n, ")
}

/// True when a followed turn gets the input row: `render.turn_input` is on, and stdin
/// and stdout are terminals.
fn row_wanted(ctx: &Context) -> bool {
    ctx.settings.render.turn_input && ctx.keys.available() && ctx.term.formats_stdout()
}

/// Stops `reader`, which no turn took over, and hands the keys that it read back to the
/// shell, then `unsent` (see [`unsent`]) on a line of its own.
async fn give_back(ctx: &Context, out: &mut Output, reader: KeyReader, unsent: &str) {
    let mut line = RowLine::default();
    for key in reader.stop_keeping().await {
        line.key(key);
    }
    let text = match (line.text(), unsent) {
        (typed, "") => typed.to_owned(),
        ("", unsent) => unsent.to_owned(),
        (typed, unsent) => format!("{typed}\n, {unsent}"),
    };
    hand_back(ctx, out, &text).await;
}

/// Hands `text` back to the shell, or shows it as a note when it cannot go there.
async fn hand_back(ctx: &Context, out: &mut Output, text: &str) {
    if let Some(note) = crate::draft::hand_back(ctx, text).await {
        let options = ctx.render_options(effective_width(ctx.screen.size()));
        out.err(&render_trace(&note, &options));
    }
}

/// Sends `prompt` and follows the turn, with the input row when `reader` holds its key
/// reader; the follow loop takes the reader. With the row, the prompts of `queued` go
/// behind it first, and the view follows them too. Returns the conversation.
async fn send_and_follow(
    ctx: &Context,
    out: &mut Output,
    origin: Origin,
    prompt: Prompt,
    reader: &mut Option<KeyReader>,
    queued: &mut VecDeque<String>,
) -> Result<ConversationId, CliError> {
    let compose = compose(&prompt);
    let client = ctx.connect(origin, prompt.context.tty.as_deref()).await?;
    let result = send_prompt(ctx, &client, prompt).await?;
    let conversation = result.conversation_id;
    let size = ctx.screen.size();
    let options = ctx.render_options(effective_width(size));
    let render = &ctx.settings.render;
    let look = Look {
        motion: render.motion,
        summary: render.turn_summary,
        progress: progress::wanted(render.progress, &ctx.term),
        diff_lines: usize::try_from(render.diff_lines).unwrap_or(usize::MAX),
    };
    let scope = result
        .route
        .as_ref()
        .map(|route| crate::format::route::scope_mark(&route.scope, ctx.home.as_deref()));
    let mut view = TurnView::new(result.turn_id, options)
        .with_home(ctx.home.clone())
        .with_look(look)
        .with_groups(ctx.settings.diff.groups.clone())
        .with_scope(scope);
    if reader.is_some() {
        view = view.with_input();
    }
    // NOTE: the line comes from the prompt.send result, so it is the reply's first line
    // even before the first event arrives; the follow loop writes it in its first frame.
    // It names where the prompt went (a switch, else the scope) with the settings note
    // after ` · `, so a turn starts with one line (DV4, DV52).
    if let Some(line) = first_line(ctx, &result) {
        let step = view.note(&line, size);
        out.err(&step.err);
        out.out(&step.out)?;
    }
    // NOTE: at most once a day for each repository; efrd decides (DV53).
    if let Some(hint) = result.route.as_ref().and_then(|route| route.hint.as_ref()) {
        let line = crate::format::route::hint_line(hint, ctx.home.as_deref());
        let step = view.note(&line, size);
        out.err(&step.err);
        out.out(&step.out)?;
    }
    if result.queued {
        view.queue();
        let step = view.note("queued behind the running turn", size);
        out.err(&step.err);
        out.out(&step.out)?;
    }
    if view.has_row() {
        queue_behind(ctx, &client, out, &mut view, &compose, conversation, queued).await?;
    }
    view.start();
    let target = Target { conversation, turn: result.turn_id, after: result.seq };
    let row = reader.take().map(|reader| Row { compose, reader, origin });
    follow::follow(ctx, &client, out, &mut view, target, row).await?;
    Ok(conversation)
}

/// Queues the prompts of `queued` behind the followed one in `conversation`, as Tab in
/// the input row does, so the view follows each one after the turn before it. When
/// efrd refuses one, it and those after it go into the row, and a note says why.
async fn queue_behind(
    ctx: &Context,
    client: &Client,
    out: &mut Output,
    view: &mut TurnView,
    compose: &Compose,
    conversation: ConversationId,
    queued: &mut VecDeque<String>,
) -> Result<(), CliError> {
    while let Some(text) = queued.front().cloned() {
        let method = Method::PromptSend(PromptSend {
            command_id: ctx.command_id(),
            conversation_id: Some(conversation),
            new_conversation: false,
            text: text.clone(),
            context: Some(compose.context.clone()),
            last_command: compose.last_command.clone().map(LastCommand::into_string),
            recent_commands: compose.recent_commands.clone(),
            settings: compose.settings.clone(),
        });
        match client.call::<PromptSendResult>(method).await {
            Ok(result) => {
                queued.pop_front();
                view.queued_prompt(result.turn_id, text, false);
            }
            Err(ClientError::Server { body }) => {
                for text in queued.drain(..) {
                    view.row_append(&text);
                }
                let note = format!("not queued: {}", crate::format::one_line(&body.message));
                let step = view.note(&note, ctx.screen.size());
                out.err(&step.err);
                out.out(&step.out)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
        recent_commands: prompt.recent_commands,
        settings: prompt.settings,
    });
    Ok(client.call(method).await?)
}

/// The first line of a reply: where the prompt went, with the settings note; `None`
/// for a prompt from no terminal whose settings came from the config.
fn first_line(ctx: &Context, result: &PromptSendResult) -> Option<String> {
    let note = result.settings.as_ref().and_then(overrides);
    match &result.route {
        Some(route) => Some(crate::format::route::line(
            route,
            note.as_deref(),
            ctx.clock.now(),
            ctx.home.as_deref(),
        )),
        None => note,
    }
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

/// The current conversation of `tty`: the one of its last prompt, `,new` or resume.
/// efrd answers the `tty` filter with that terminal's conversations, the current one
/// first, in one page; an older efrd ignores it and lists every conversation, which
/// the scan of the pages reads as before.
pub(crate) async fn active_conversation(
    client: &Client,
    tty: &str,
) -> Result<ConversationId, CliError> {
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let method = Method::ConversationsList(ConversationsList {
            cursor: cursor.take(),
            limit: Some(PAGE),
            tty: Some(tty.to_owned()),
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
/// terminal on stdin. Either way its desktop variables are the six names of this
/// process's environment ([`Context::desktop`]), which the shell passed on, never
/// what the JSON says.
fn shell_context(
    ctx: &Context,
    json: Option<(&'static str, String)>,
) -> Result<ShellContext, CliError> {
    let mut context = match json {
        Some((input, json)) => serde_json::from_str(&json)
            .map_err(|source| CliError::InvalidContext { input, source })?,
        None => {
            let mut context = ShellContext::new(ctx.cwd.clone().unwrap_or_default());
            context.tty.clone_from(&ctx.tty);
            // NOTE: typed by hand in a terminal, the shell is efr's parent when it runs
            // on the same terminal (VR1-9); efrd then lets the slot go once it ends.
            context.shell_pid = ctx.tty.as_deref().and_then(shell::parent_on);
            context
        }
    };
    context.desktop.clone_from(&ctx.desktop);
    Ok(context)
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
