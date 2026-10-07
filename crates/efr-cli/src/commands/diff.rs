//! `efr diff`: what a turn changed in the files of its roots (its project and
//! `$SCRATCH`), from the daemon's snapshots (`conversation.diff`).
//!
//! Without `--turn` it shows the last turn of this terminal's conversation (the newest
//! whose `tty` is the terminal on stdin), else of the newest conversation, and a muted
//! line on stderr says which. On a terminal the diff is written to the scrollback in
//! the `diff.*` roles with the files' syntax colours, then a muted line counts the files
//! and the lines. Elsewhere stdout holds the diff alone, as the daemon sent it, so it
//! can go to `git apply` or a pager. `--stat` lists the files instead. A turn that
//! changed nothing says so on stderr, and a turn without a snapshot is an error.

use std::fmt::Write as _;

use efr_client::ClientError;
use efr_protocol::{ConversationDiff, ConversationDiffResult, ErrorCode, Method, Origin};
use efr_render::{RenderOptions, render};

use crate::cli::DiffArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, changes};
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &DiffArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let (conversation_id, picked) = match (&args.conversation, args.turn) {
        (Some(query), _) => (Some(super::history::resolve(&client, query).await?), None),
        // NOTE: a turn id names its conversation, so the daemon finds it.
        (None, Some(_)) => (None, None),
        (None, None) => match super::history::pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => (Some(id), Some(picked)),
            None => return Err(CliError::NoConversation),
        },
    };
    let method = Method::ConversationDiff(ConversationDiff {
        conversation_id,
        turn_id: args.turn,
        stat: args.stat,
    });
    let result: ConversationDiffResult = match client.call(method).await {
        Ok(result) => result,
        Err(ClientError::Server { body }) if body.code == ErrorCode::NotFound => {
            let what = match (args.turn, conversation_id) {
                (Some(turn), _) => format!("turn {turn}"),
                (None, Some(id)) => format!("the last turn of conversation {id}"),
                (None, None) => "the last turn".to_owned(),
            };
            return Err(CliError::NoDiff { what, reason: body.message });
        }
        Err(error) => return Err(error.into()),
    };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    if let Some(picked) = picked {
        let line =
            format!("the last turn of {}; efr diff --turn <id> shows another", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
    if changes::turn_line(&result.changes).is_none() {
        out.err(&format!("turn {} changed no files\n", result.turn_id));
        return Ok(());
    }
    out.out(&shown(&result, args.stat, &options))
}

/// What `efr diff` writes to stdout for `result`: the list of files with `stat` or when
/// the daemon sent no diff, else the diff. On a terminal the diff is painted and a
/// muted line counts the files and the lines; elsewhere it is the daemon's text, with
/// control characters other than tabs and newlines as visible stand-ins.
pub(crate) fn shown(
    result: &ConversationDiffResult,
    stat: bool,
    options: &RenderOptions,
) -> String {
    let diff = match result.diff.as_deref() {
        Some(diff) if !stat => diff,
        _ => return changes::stat(&result.changes, options),
    };
    if !options.is_terminal() {
        let mut text = format::diff_text(diff).into_owned();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        return text;
    }
    let (body, cut) = changes::split_cut(diff);
    let mut out = render(&format::code_block("diff", body), options);
    if cut > 0 {
        let _ = writeln!(out, "{}", format::paint(&changes::more_lines(cut), Tone::Dim, options));
    }
    if let Some(line) = changes::turn_line(&result.changes) {
        let _ = write!(out, "\n{}\n", format::paint(&line, Tone::Dim, options));
    }
    out
}

#[cfg(test)]
mod tests;
