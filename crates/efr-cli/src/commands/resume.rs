//! `efr resume [N | conversation]` (`conversations.recent`, `conversation.resume`): list
//! the recent conversations, or make one this terminal's conversation again. The zsh
//! plugin runs it for `,resume`.
//!
//! Without an argument it prints the list of `format/resume.rs`, inline, the
//! conversations of the current scope first. An argument of one or two digits is N in
//! the list that efrd computes at that moment; any other argument is a conversation id
//! or at least 4 characters of one, and exactly one conversation must match. The line
//! after the resume names the title, so a list that changed in between shows at once.
//!
//! The terminal and the working directory come from the plugin's `EFR_CONTEXT`, else
//! from the process (its cwd and the terminal on stdin), as for `efr send`. When
//! another open terminal holds the conversation, efrd moves nothing: on a terminal efr
//! asks on one line (`y` moves it, anything else or Esc does not); in a pipe it fails
//! with the hint `efr resume --take N`. `--take` moves it with no question. A daemon
//! from before this command refuses the methods; efr then says to restart it.

use efr_protocol::{
    ConversationId, ConversationResume, ConversationResumeResult, ConversationsList,
    ConversationsListResult, ConversationsRecent, ConversationsRecentResult, ErrorCode, Method,
    Origin, ShellContext,
};
use efr_transport::client::{Client, ClientError};
use serde::de::DeserializeOwned;

use crate::cli::ResumeArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, resume};
use crate::keys::Key;
use crate::live::effective_width;
use crate::output::Output;

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

/// What the argument of `efr resume` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// N, the row of the list, from 1.
    Row(usize),
    /// A conversation id, or the start of one.
    Id(String),
}

impl Target {
    /// One or two digits are a row; anything else is an id or the start of one.
    pub(crate) fn of(argument: &str) -> Target {
        let digits =
            (1..=2).contains(&argument.len()) && argument.bytes().all(|byte| byte.is_ascii_digit());
        match argument.parse::<usize>() {
            Ok(n) if digits => Target::Row(n),
            _ => Target::Id(argument.to_owned()),
        }
    }
}

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &ResumeArgs,
) -> Result<(), CliError> {
    let Handover { context, origin, .. } = Handover::read(ctx, None, None, &[])?;
    let client = ctx.connect(origin, context.tty.as_deref()).await?;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let plugin = origin == Origin::Shell;
    let Some(argument) = &args.conversation else {
        let recent = recent(&client, &context).await?;
        return out.out(&resume::list(&recent, ctx.clock.now(), plugin, &options));
    };
    if context.tty.is_none() {
        return Err(CliError::ResumeNeedsTerminal);
    }
    let conversation_id = match Target::of(argument) {
        Target::Row(n) => {
            let recent = recent(&client, &context).await?;
            let count = recent.conversations.len();
            match n.checked_sub(1).and_then(|index| recent.conversations.get(index)) {
                Some(row) => row.summary.id,
                None => return Err(CliError::NoResumeRow { n, count }),
            }
        }
        Target::Id(query) => resolve(&client, &query).await?,
    };
    let mut take = args.take;
    loop {
        let params = ConversationResume {
            command_id: ctx.command_id(),
            conversation_id,
            context: context.clone(),
            take,
        };
        match call(&client, Method::ConversationResume(params)).await? {
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
                    return Err(CliError::ResumeHeld { title, tty, target: argument.clone() });
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
async fn call<T: DeserializeOwned>(client: &Client, method: Method) -> Result<T, CliError> {
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
