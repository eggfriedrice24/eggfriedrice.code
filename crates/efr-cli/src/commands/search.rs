//! `efr search <words>` (`conversations.search`, `conversation.search`): the
//! conversations whose titles or text match some words, best first.
//!
//! The plain list is in `format/search.rs`: the cells of the list of `efr resume`, the
//! start of each id that `efr resume` takes, and a muted line with the best match.
//! `--json` prints the daemon's answer as it came, with the measures of the search.
//!
//! The picker of `,resume` runs two hidden forms in fzf, on each key and each move of
//! the focus. `--picker` prints the rows for the query in `FZF_QUERY`, and `--preview`
//! prints the matching lines of one conversation, for the preview window. Neither takes
//! the query as an argument, so no other local user can read it in the process list.
//! Each one first waits a short time on the clock (50 ms and 30 ms): fzf ends a command
//! when the next key comes, so a fast typist starts no search per key. They write
//! nothing to stderr, which fzf would draw over its rows: a failure becomes the note of
//! the header, or a muted line of the preview, and the exit code is 0.

use std::time::Duration;

use efr_protocol::{
    ConversationId, ConversationSearch, ConversationSearchResult, ConversationsSearch,
    ConversationsSearchResult, ErrorCode, Method, Origin,
};
use efr_render::RenderOptions;
use efr_transport::client::ClientError;
use serde_json::Value;

use crate::cli::SearchArgs;
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::format::search;
use crate::live::effective_width;
use crate::output::Output;

use super::resume::call;
use super::send::Handover;

/// The most code points of a query that efrd takes.
pub(crate) const MAX_QUERY_CHARS: usize = 256;

/// The most words and phrases of a query that efrd takes.
pub(crate) const MAX_QUERY_TERMS: usize = 16;

/// How long `--picker` waits before it connects: a key within this time makes fzf end
/// it first.
pub(crate) const PICKER_WAIT: Duration = Duration::from_millis(50);

/// How long `--preview` waits before it connects, for a fast move of the focus.
pub(crate) const PREVIEW_WAIT: Duration = Duration::from_millis(30);

/// The rows of the plain list unless `--limit` says otherwise.
const LIST_LIMIT: u32 = 10;

/// The rows that the picker asks for.
pub(crate) const PICKER_LIMIT: u32 = 50;

/// The width of the preview when fzf does not export it.
const PREVIEW_COLUMNS: u16 = 80;

/// The note of the picker when no daemon answers.
const NO_DAEMON: &str = "efrd does not answer";

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &SearchArgs,
) -> Result<(), CliError> {
    if args.picker {
        return picker(ctx, out, args.tty.as_deref()).await;
    }
    if let Some(id) = &args.preview {
        return preview(ctx, out, id, args.tty.as_deref()).await;
    }
    let query = args.words.join(" ");
    if query.trim().is_empty() {
        return Err(CliError::SearchWithoutWords);
    }
    check(&query)?;
    let Handover { context, origin, .. } = Handover::read(ctx, None, None, &[])?;
    let client = ctx.connect(origin, context.tty.as_deref()).await?;
    let method = Method::ConversationsSearch(ConversationsSearch {
        query,
        tty: context.tty.clone(),
        pwd: Some(context.pwd.clone()),
        limit: Some(args.limit.unwrap_or(LIST_LIMIT)),
        stats: args.json,
    });
    if args.json {
        // NOTE: the answer as it came, so a member that a newer efrd adds is kept.
        let answer: Value = call(&client, method).await?;
        let mut text = serde_json::to_string_pretty(&answer)
            .map_err(|error| CliError::Output { source: error.into() })?;
        text.push('\n');
        return out.out(&text);
    }
    let result: ConversationsSearchResult = call(&client, method).await?;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    out.out(&search::list(&result, ctx.clock.now(), origin == Origin::Shell, &options))
}

/// `--picker`: the header and the rows for the query in `FZF_QUERY`.
async fn picker(ctx: &Context, out: &mut Output, tty: Option<&str>) -> Result<(), CliError> {
    let query = ctx.fzf_query.as_ref().map_or("", |query| query.0.as_str());
    let options = hidden_options(ctx, 0);
    let text = match check(query) {
        Ok(()) => {
            ctx.clock.sleep(PICKER_WAIT).await;
            match rows(ctx, query, tty).await {
                Ok(result) => search::picker_rows(&result, ctx.clock.now(), &options),
                Err(error) => search::picker_header(note(&error).as_deref(), &options),
            }
        }
        Err(error) => search::picker_header(note(&error).as_deref(), &options),
    };
    out.out(&text)
}

/// The rows of `conversations.search` for the picker.
async fn rows(
    ctx: &Context,
    query: &str,
    tty: Option<&str>,
) -> Result<ConversationsSearchResult, CliError> {
    let client = ctx.connect(Origin::Cli, tty).await?;
    let method = Method::ConversationsSearch(ConversationsSearch {
        query: query.to_owned(),
        tty: tty.map(str::to_owned),
        pwd: ctx.cwd.clone(),
        limit: Some(PICKER_LIMIT),
        stats: false,
    });
    call(&client, method).await
}

/// `--preview <id>`: the matching lines of the conversation `id` for the query in
/// `FZF_QUERY`, cut to the width of the preview window. An empty id, which fzf gives
/// for an empty list, prints nothing.
async fn preview(
    ctx: &Context,
    out: &mut Output,
    id: &str,
    tty: Option<&str>,
) -> Result<(), CliError> {
    if id.trim().is_empty() {
        return Ok(());
    }
    let query = ctx.fzf_query.as_ref().map_or("", |query| query.0.as_str());
    let width = ctx.preview_columns.unwrap_or(PREVIEW_COLUMNS);
    let options = hidden_options(ctx, width);
    let columns = usize::from(width);
    let Ok(conversation_id) = id.trim().parse::<ConversationId>() else {
        let line = "the row names no conversation";
        return out.out(&search::preview_failure(line, columns, &options));
    };
    let answer = match check(query) {
        Ok(()) => {
            ctx.clock.sleep(PREVIEW_WAIT).await;
            hits(ctx, conversation_id, query, tty).await
        }
        Err(error) => Err(error),
    };
    let text = match answer {
        Ok(result) => search::preview(&result, query, ctx.clock.now(), columns, &options),
        Err(error) => match note(&error) {
            Some(note) => search::preview_failure(&note, columns, &options),
            None => String::new(),
        },
    };
    out.out(&text)
}

/// The hits of `conversation.search` for the preview.
async fn hits(
    ctx: &Context,
    conversation_id: ConversationId,
    query: &str,
    tty: Option<&str>,
) -> Result<ConversationSearchResult, CliError> {
    let client = ctx.connect(Origin::Cli, tty).await?;
    let method = Method::ConversationSearch(ConversationSearch {
        conversation_id,
        query: query.to_owned(),
        limit: None,
    });
    call(&client, method).await
}

/// The render options of the hidden forms: fzf reads their colours (`--ansi`) also
/// though stdout is a pipe, and `TERM=dumb` or `NO_COLOR` turn them off.
pub(crate) fn hidden_options(ctx: &Context, width: u16) -> RenderOptions {
    let terminal = ctx.term.term.as_deref() != Some("dumb");
    ctx.term.render_options(effective_width_of(width), &ctx.settings).with_terminal(terminal)
}

/// `width`, or the fallback width when it is unknown.
fn effective_width_of(width: u16) -> u16 {
    effective_width(crate::terminal::Size { cols: width, rows: 0 })
}

/// The note that the picker shows for `error`, with no text of the query: `None` for a
/// busy efrd, which the next key asks again.
pub(crate) fn note(error: &CliError) -> Option<String> {
    match error {
        CliError::Client(ClientError::Server { body }) if body.code == ErrorCode::Busy => None,
        CliError::Client(
            ClientError::DaemonNotRunning { .. }
            | ClientError::Connect { .. }
            | ClientError::ConnectTimedOut { .. }
            | ClientError::HelloTimedOut { .. }
            | ClientError::Closed
            | ClientError::Io { .. },
        ) => Some(NO_DAEMON.to_owned()),
        other => Some(efr_stdx::with_causes(other)),
    }
}

/// What a hidden form prints when its context could not be gathered, such as without
/// the efr directories: the header with the note for the picker, nothing for the
/// preview. Nothing goes to stderr, and the exit code is 0.
pub(crate) fn without_context(args: &SearchArgs, error: &CliError, out: &mut Output) -> Exit {
    let options = RenderOptions::new(PREVIEW_COLUMNS);
    let note = note(error);
    let text = match (&args.preview, note) {
        (Some(_), Some(note)) => search::preview_failure(&note, PREVIEW_COLUMNS.into(), &options),
        (Some(_), None) => String::new(),
        (None, note) => search::picker_header(note.as_deref(), &options),
    };
    match out.out(&text) {
        Ok(()) => Exit::Success,
        Err(error) => error.exit(),
    }
}

/// Fails when `query` is longer than efrd takes, before any call: more than
/// [`MAX_QUERY_CHARS`] code points or [`MAX_QUERY_TERMS`] words and phrases, counted
/// as efrd counts them.
pub(crate) fn check(query: &str) -> Result<(), CliError> {
    let trimmed = query.trim();
    let chars = trimmed.chars().count();
    if chars > MAX_QUERY_CHARS {
        return Err(CliError::QueryTooLong { chars });
    }
    let words = terms(trimmed);
    if words > MAX_QUERY_TERMS {
        return Err(CliError::QueryTooManyWords { words });
    }
    Ok(())
}

/// The words and phrases of `query` by efrd's rule: white space splits words, and a
/// `"` starts or ends a phrase, also inside a word; an open phrase at the end counts.
pub(crate) fn terms(query: &str) -> usize {
    let mut count = 0;
    let mut current = false;
    let mut in_phrase = false;
    let mut end = |current: &mut bool| {
        if std::mem::take(current) {
            count += 1;
        }
    };
    for c in query.chars() {
        if c == '"' {
            end(&mut current);
            in_phrase = !in_phrase;
        } else if c.is_whitespace() && !in_phrase {
            end(&mut current);
        } else if !c.is_whitespace() {
            current = true;
        }
    }
    end(&mut current);
    count
}

#[cfg(test)]
mod tests;
