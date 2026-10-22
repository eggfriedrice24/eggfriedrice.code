//! `efr resume [N | conversation | words]` (`conversations.search`,
//! `conversations.recent`, `conversation.resume`): find a recent conversation, or make
//! one this terminal's conversation again. The zsh plugin runs it for `,resume`.
//!
//! Without an argument it opens the picker of `picker.rs` on a terminal: fzf, inline
//! below the command line, with the recent conversations first and a live search of
//! their titles and text as the user types. Enter continues the chosen one here, Ctrl-O
//! prints it as `efr history` does, Esc closes and changes nothing. Without the picker
//! (no fzf, an older one, a pipe, `TERM=dumb`, `resume.picker = "list"`, an older efrd)
//! it prints the list of `format/resume.rs`, inline, the conversations of the current
//! scope first, byte for byte as before the picker; a reason that the user may not
//! expect, such as an fzf that is too old, adds one muted line after it.
//!
//! An argument of one or two digits is N in the list that efrd computes at that
//! moment. A whole id, or at least 4 characters of hex digits and `-` that start
//! exactly one id, is that conversation; two or more matches are an error that names
//! them. Anything else is a search: the picker opens with the words as its first
//! query, or without it the best matches print (`format/search.rs`). The line after a
//! resume names the title, so a list that changed in between shows at once.
//!
//! The words come from the arguments, else from `EFR_RESUME_WORDS`, where the plugin
//! puts the words of `,resume` so that they never show in the process list. fzf gets
//! them in its environment, never in its arguments.
//!
//! The terminal and the working directory come from the plugin's `EFR_CONTEXT`, else
//! from the process (its cwd and the terminal on stdin), as for `efr send`. When
//! another open terminal holds the conversation, efrd moves nothing: on a terminal efr
//! asks on one line (`y` moves it, anything else or Esc does not); in a pipe it fails
//! with the hint `efr resume --take N`. `--take` moves the conversation of N or an id
//! with no question. A daemon from before this command refuses the methods; efr then
//! says to restart it.

use efr_protocol::{
    ConversationHistory, ConversationHistoryResult, ConversationId, ConversationResume,
    ConversationResumeResult, ConversationsList, ConversationsListResult, ConversationsRecent,
    ConversationsRecentResult, ConversationsSearch, ConversationsSearchResult, ErrorCode, Method,
    Origin, ShellContext,
};
use efr_render::RenderOptions;
use efr_stdx::env::Var;
use efr_transport::client::{Client, ClientError};
use serde::de::DeserializeOwned;

use crate::cli::ResumeArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, resume};
use crate::keys::Key;
use crate::live::effective_width;
use crate::output::Output;
use crate::picker::{Fzf, NoPicker, Outcome, PickRequest, PickerLook, Stops, fzf_colours};
use crate::terminal::Background;

use super::history::{self, Shown};
use super::search;
use super::send::Handover;

/// What an older efrd answers for a method that it does not know.
const UNKNOWN_FRAME: &str = "the frame is not a valid client frame";

/// The fewest characters of an id start.
const MIN_PREFIX: usize = 4;
/// Conversations asked for per page while matching an id start.
const PAGE: u32 = 200;
/// Pages read at most while matching an id start.
const MAX_PAGES: usize = 50;
/// The matches that an ambiguous id start names.
const SHOWN_MATCHES: usize = 5;
/// The rows of the plain list of a search without the picker, as `efr search` prints.
const LIST_LIMIT: u32 = 10;

/// What the words of `efr resume` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// N, the row of the list, from 1.
    Row(usize),
    /// A conversation id, or the start of one.
    Id(String),
    /// Words to search for.
    Search(String),
}

impl Target {
    /// One or two digits are a row; a whole id, or at least 4 hex digits and `-`, are
    /// an id or the start of one; anything else, and more than one word, is a search.
    pub(crate) fn of(words: &str) -> Target {
        let words = words.trim();
        if words.contains(char::is_whitespace) {
            return Target::Search(words.to_owned());
        }
        let digits =
            (1..=2).contains(&words.len()) && words.bytes().all(|byte| byte.is_ascii_digit());
        match words.parse::<usize>() {
            Ok(n) if digits => return Target::Row(n),
            _ => {}
        }
        let id_start = words.len() >= MIN_PREFIX
            && words.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-');
        if id_start || words.parse::<ConversationId>().is_ok() {
            Target::Id(words.to_owned())
        } else {
            Target::Search(words.to_owned())
        }
    }
}

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &ResumeArgs,
) -> Result<(), CliError> {
    let Handover { context, origin, .. } = Handover::read(ctx, None, None, &[])?;
    let words = words(ctx, args)?;
    if args.take && words.is_none() {
        return Err(CliError::ResumeTakeWithoutTarget);
    }
    let plugin = origin == Origin::Shell;
    let target = words.as_deref().map(Target::of);
    let query = match target {
        None => None,
        Some(Target::Search(query)) => Some(query),
        Some(Target::Row(n)) => {
            let client = ctx.connect(origin, context.tty.as_deref()).await?;
            if context.tty.is_none() {
                return Err(CliError::ResumeNeedsTerminal);
            }
            let recent = recent(&client, &context).await?;
            let count = recent.conversations.len();
            let id = match n.checked_sub(1).and_then(|index| recent.conversations.get(index)) {
                Some(row) => row.summary.id,
                None => return Err(CliError::NoResumeRow { n, count }),
            };
            let target = n.to_string();
            return resume_loop(ctx, out, &client, &context, id, args.take, &target).await;
        }
        Some(Target::Id(query)) => {
            let client = ctx.connect(origin, context.tty.as_deref()).await?;
            match resolve(&client, &query).await {
                Ok(id) => {
                    if context.tty.is_none() {
                        return Err(CliError::ResumeNeedsTerminal);
                    }
                    return resume_loop(ctx, out, &client, &context, id, args.take, &query).await;
                }
                // NOTE: hex words that start no id, such as `cafe`, are a search.
                Err(CliError::ConversationNotFound { .. }) => {
                    search::check(&query)?;
                    let found = match choose(ctx, &context) {
                        Ok(()) => ctx.picker.find().await,
                        Err(no) => Err(no),
                    };
                    return pick_or_list(ctx, out, &client, &context, plugin, Some(query), found)
                        .await;
                }
                Err(error) => return Err(error),
            }
        }
    };
    if let Some(query) = &query {
        search::check(query)?;
    }
    // NOTE: the PATH walk and `fzf --version` run while efr connects.
    let connect = ctx.connect(origin, context.tty.as_deref());
    let find = async {
        choose(ctx, &context)?;
        ctx.picker.find().await
    };
    let (client, found) = tokio::join!(connect, find);
    pick_or_list(ctx, out, &client?, &context, plugin, query, found).await
}

/// The words of `efr resume`: its arguments joined with a blank, else
/// `EFR_RESUME_WORDS`; `None` when they are blank.
fn words(ctx: &Context, args: &ResumeArgs) -> Result<Option<String>, CliError> {
    let text = if args.words.is_empty() {
        let var = ctx.env.var(Var::ResumeWords);
        var.map_err(|source| CliError::Environment { source })?.unwrap_or_default()
    } else {
        args.words.join(" ")
    };
    let text = text.trim();
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

/// Whether the terminal and the settings allow the picker, before efr looks for fzf:
/// stdin and stdout are terminals, `TERM` is set and not `dumb`, the context names a
/// terminal and `resume.picker` is not `list`.
fn choose(ctx: &Context, context: &ShellContext) -> Result<(), NoPicker> {
    if !(ctx.term.stdin_tty && ctx.term.stdout_tty) {
        return Err(NoPicker::NotATerminal);
    }
    if matches!(ctx.term.term.as_deref(), None | Some("dumb")) {
        return Err(NoPicker::DumbTerminal);
    }
    if context.tty.is_none() {
        return Err(NoPicker::NoTerminalInContext);
    }
    if ctx.settings.resume.picker == efr_config::ResumePicker::List {
        return Err(NoPicker::ConfigList);
    }
    Ok(())
}

/// Opens the picker with `found`, or prints the list: the numbered list without a
/// query, the best matches of `query` with one.
async fn pick_or_list(
    ctx: &Context,
    out: &mut Output,
    client: &Client,
    context: &ShellContext,
    plugin: bool,
    query: Option<String>,
    found: Result<Fzf, NoPicker>,
) -> Result<(), CliError> {
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let chosen = found.and_then(|fzf| {
        if client.hello().capabilities.search != Some(true) {
            return Err(NoPicker::NoSearch);
        }
        let look = look(ctx, context, &options).ok_or(NoPicker::UnsafePath)?;
        Ok((fzf, look))
    });
    let (fzf, look) = match chosen {
        Ok(chosen) => chosen,
        Err(no) => {
            return list(ctx, out, client, context, plugin, query.as_deref(), no.note()).await;
        }
    };
    let rows = first_rows(ctx, client, context, query.as_deref()).await;
    let request = PickRequest {
        rows: rows.into_bytes(),
        query: query.clone(),
        look,
        cwd: context.pwd.clone(),
    };
    let stops = Stops { interrupt: ctx.interrupt.wait(), terminate: ctx.terminate.wait() };
    match ctx.picker.run(fzf, request, stops).await? {
        Outcome::Resume(id) => {
            let target = id.to_string();
            resume_loop(ctx, out, client, context, id, false, &target).await
        }
        Outcome::Print(id) => print(ctx, out, client, id).await,
        Outcome::Closed => Ok(()),
        Outcome::Interrupted => Err(CliError::Interrupted),
        Outcome::Failed(status) => {
            let note = Outcome::failure_note(status);
            list(ctx, out, client, context, plugin, query.as_deref(), Some(note)).await
        }
    }
}

/// The look of the picker: the commands run this efr on the context's terminal, and
/// fzf takes its colours from the colour roles. `None` when the path of efr or the
/// name of the terminal cannot go into a command of fzf.
fn look(ctx: &Context, context: &ShellContext, options: &RenderOptions) -> Option<PickerLook> {
    let efr = ctx.picker.efr()?;
    let tty = context.tty.as_deref()?;
    let light = ctx.settings.background == Some(Background::Light);
    PickerLook::new(efr.as_os_str(), tty, fzf_colours(options, light))
}

/// The first rows of the picker, as `efr search --picker` prints them for `query`:
/// the recent conversations without one. A failed search is the header with its note,
/// and the next key asks again.
async fn first_rows(
    ctx: &Context,
    client: &Client,
    context: &ShellContext,
    query: Option<&str>,
) -> String {
    let options = search::hidden_options(ctx, 0);
    let method = Method::ConversationsSearch(ConversationsSearch {
        query: query.unwrap_or_default().to_owned(),
        tty: context.tty.clone(),
        pwd: Some(context.pwd.clone()),
        limit: Some(search::PICKER_LIMIT),
        stats: false,
    });
    match call::<ConversationsSearchResult>(client, method).await {
        Ok(result) => format::search::picker_rows(&result, ctx.clock.now(), &options),
        Err(error) => format::search::picker_header(search::note(&error).as_deref(), &options),
    }
}

/// The list without the picker, then `note` as a muted line: the numbered list of
/// `conversations.recent` without a query, the best matches of `query` with one.
async fn list(
    ctx: &Context,
    out: &mut Output,
    client: &Client,
    context: &ShellContext,
    plugin: bool,
    query: Option<&str>,
    note: Option<String>,
) -> Result<(), CliError> {
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let mut text = match query {
        None => {
            let recent = recent(client, context).await?;
            resume::list(&recent, ctx.clock.now(), plugin, &options)
        }
        Some(query) => {
            let method = Method::ConversationsSearch(ConversationsSearch {
                query: query.to_owned(),
                tty: context.tty.clone(),
                pwd: Some(context.pwd.clone()),
                limit: Some(LIST_LIMIT),
                stats: false,
            });
            let result: ConversationsSearchResult = call(client, method).await?;
            format::search::list(&result, ctx.clock.now(), plugin, &options)
        }
    };
    if let Some(note) = note {
        text.push_str(&format::paint(&format::one_line(&note), Tone::Dim, &options));
        text.push('\n');
    }
    out.out(&text)
}

/// Ctrl-O in the picker: the conversation `id` as `efr history <id>` prints it, the
/// newest page of its events. Nothing moves.
async fn print(
    ctx: &Context,
    out: &mut Output,
    client: &Client,
    id: ConversationId,
) -> Result<(), CliError> {
    let method = Method::ConversationHistory(ConversationHistory {
        conversation_id: id,
        cursor: None,
        limit: None,
    });
    let page: ConversationHistoryResult = call(client, method).await?;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let shown = Shown {
        options: &options,
        verbose: false,
        home: ctx.home.as_deref(),
        groups: &ctx.settings.diff.groups,
    };
    out.out(&history::transcript(id, &page, &shown))
}

/// Makes `id` this terminal's conversation, and asks first on a terminal when another
/// open terminal holds it; `target` is what `--take` would take in the hint of a pipe.
async fn resume_loop(
    ctx: &Context,
    out: &mut Output,
    client: &Client,
    context: &ShellContext,
    conversation_id: ConversationId,
    take: bool,
    target: &str,
) -> Result<(), CliError> {
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let mut take = take;
    loop {
        let params = ConversationResume {
            command_id: ctx.command_id(),
            conversation_id,
            context: context.clone(),
            take,
        };
        match call(client, Method::ConversationResume(params)).await? {
            ConversationResumeResult::Resumed {
                scope,
                title,
                updated_at,
                here,
                continues_here,
                ..
            } => {
                let resumed = resume::Resumed {
                    title: title.as_deref(),
                    scope: &scope,
                    updated_at,
                    here,
                    continues_here,
                };
                let line = resume::resumed_line(&resumed, ctx.clock.now(), ctx.home.as_deref());
                return out.out(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
            }
            ConversationResumeResult::Held { tty, title } if !take => {
                if !ctx.keys.available() {
                    return Err(CliError::ResumeHeld { title, tty, target: target.to_owned() });
                }
                let question = resume::held_question(title.as_deref(), &tty);
                out.out(&format!("{}\n", format::paint(&question, Tone::Attention, &options)))?;
                if !yes(ctx).await? {
                    let line = "nothing moved";
                    out.err(&format!("{}\n", format::paint(line, Tone::Dim, &options)));
                    return Ok(());
                }
                take = true;
            }
            _ => {
                let line =
                    "efrd answered in a way that this efr does not know; nothing changed here";
                out.err(&format!("{}\n", format::paint(line, Tone::Dim, &options)));
                return Ok(());
            }
        }
    }
}

/// The list of `conversations.recent` for the terminal and directory of `context`.
async fn recent(
    client: &Client,
    context: &ShellContext,
) -> Result<ConversationsRecentResult, CliError> {
    let method = Method::ConversationsRecent(ConversationsRecent {
        tty: context.tty.clone(),
        pwd: Some(context.pwd.clone()),
        limit: None,
    });
    call(client, method).await
}

/// Calls `method`; the refusal of a daemon that does not know it says to restart it.
pub(crate) async fn call<T: DeserializeOwned>(
    client: &Client,
    method: Method,
) -> Result<T, CliError> {
    match client.call(method).await {
        Ok(result) => Ok(result),
        Err(ClientError::Server { body })
            if body.code == ErrorCode::Invalid && body.message == UNKNOWN_FRAME =>
        {
            Err(CliError::OlderDaemon)
        }
        Err(error) => Err(error.into()),
    }
}

/// The conversation that `query` names: a whole id, or the start of exactly one
/// conversation's id. More than one match fails with the matches.
pub(crate) async fn resolve(client: &Client, query: &str) -> Result<ConversationId, CliError> {
    if let Ok(id) = query.parse::<ConversationId>() {
        return Ok(id);
    }
    let prefix = query.to_ascii_lowercase();
    if prefix.len() < MIN_PREFIX {
        return Err(CliError::ConversationNotFound { query: query.to_owned() });
    }
    let mut matches = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let method = Method::ConversationsList(ConversationsList {
            cursor: cursor.take(),
            limit: Some(PAGE),
            tty: None,
        });
        let page: ConversationsListResult = client.call(method).await?;
        matches.extend(
            page.conversations
                .into_iter()
                .filter(|summary| summary.id.to_string().starts_with(&prefix)),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    match matches.as_slice() {
        [one] => Ok(one.id),
        [] => Err(CliError::ConversationNotFound { query: query.to_owned() }),
        _ => {
            let mut named: Vec<String> = matches
                .iter()
                .take(SHOWN_MATCHES)
                .map(|summary| match &summary.title {
                    Some(title) => format!("{} ({})", summary.id, format::one_line(title)),
                    None => summary.id.to_string(),
                })
                .collect();
            if matches.len() > SHOWN_MATCHES {
                named.push(format!("{} more", matches.len() - SHOWN_MATCHES));
            }
            Err(CliError::ResumeAmbiguous { query: query.to_owned(), matches: named })
        }
    }
}

/// Reads keys until one answers the question: `y` says yes; `n`, Enter and Esc say no,
/// the default. Ctrl+C stops `efr`.
async fn yes(ctx: &Context) -> Result<bool, CliError> {
    let mut reader = ctx.keys.start()?;
    let mut interrupt = ctx.interrupt.wait();
    let answer = loop {
        tokio::select! {
            key = reader.next() => match key {
                Some(Key::Byte(b'y' | b'Y')) => break Ok(true),
                Some(Key::Byte(b'n' | b'N' | b'\r' | b'\n')) | Some(Key::Esc) | None => {
                    break Ok(false);
                }
                Some(Key::Byte(_)) => {}
            },
            () = &mut interrupt => break Err(CliError::Interrupted),
        }
    };
    reader.stop().await;
    answer
}

#[cfg(test)]
mod tests;
