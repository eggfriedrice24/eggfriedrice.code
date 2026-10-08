//! `efr compact [focus]`: compact the model's context of a conversation now
//! (`conversation.compact`). The zsh plugin runs it for `,compact [focus]`.
//!
//! efrd prunes old tool output and writes a summary of the history before the newest
//! turns, which stay word for word; the rules are in the README of `efr-conversation`,
//! section "Context". It never starts a turn, and it refuses (`conflict`) while a turn
//! of the conversation runs, because a turn compacts on its own when it needs to.
//!
//! The focus says what the summary must keep, in the user's words. The plugin hands it
//! over in `EFR_PROMPT`, as it does a prompt, so no other user can read it in the
//! command line; the words win over the variable. Without `--conversation`, the
//! command compacts this terminal's conversation (the newest whose `tty` is the terminal
//! on stdin), else the newest of all, and a muted line on stderr says which.
//!
//! On a terminal, a status row says `compacting context` with the spinner and the time
//! while efrd works, and goes when the answer comes. Then one line says what came of
//! it, as a turn shows a compaction: `context compacted (efr compact): 140k -> 19k
//! tokens, kept 3 turns, summary 3.2k`, muted on a terminal and plain elsewhere.
//! Ctrl+C stops the wait (exit 130), not the compaction.

use std::time::Duration;

use efr_client::ClientError;
use efr_protocol::{ConversationCompact, ConversationCompactResult, Method, Origin};
use efr_render::{RenderOptions, Role};
use efr_stdx::env::Var;
use jiff::Timestamp;

use crate::cli::CompactArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::follow::{TICK, since_then, spinner};
use crate::format::{self, Tone, context};
use crate::live::effective_width;
use crate::output::Output;

/// What the status row says while efrd compacts.
const WORDS: &str = "compacting context";

/// The note when Ctrl+C stops the wait.
const STOPPED: &str = "stopped waiting; efrd may still compact the conversation";

/// The time from which the status row shows how long the compaction has run.
const SHOW_TIME: Duration = Duration::from_secs(1);

/// Erases the row that the cursor is on.
const ERASE_ROW: &str = "\r\x1b[2K";

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &CompactArgs,
) -> Result<(), CliError> {
    let focus = focus(ctx, &args.focus)?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let (conversation_id, picked) = match &args.conversation {
        Some(query) => (super::history::resolve(&client, query).await?, None),
        None => match super::history::pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => (id, Some(picked)),
            None => return Err(CliError::NothingToCompact),
        },
    };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    if let Some(picked) = picked {
        let line = format!("{}; efr compact --conversation <id> compacts another", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
    let method = Method::ConversationCompact(ConversationCompact {
        command_id: ctx.command_id(),
        conversation_id,
        focus,
    });
    let result: ConversationCompactResult = wait(ctx, out, &options, client.call(method)).await?;
    out.out(&context::compacted_rows(&result.compaction, &options))
}

/// The focus: the words joined with spaces, else `EFR_PROMPT`; `None` when it is blank.
fn focus(ctx: &Context, words: &[String]) -> Result<Option<String>, CliError> {
    let text = if words.is_empty() {
        ctx.env.var(Var::Prompt).map_err(|source| CliError::Environment { source })?
    } else {
        Some(words.join(" "))
    };
    Ok(text.map(|text| text.trim().to_owned()).filter(|text| !text.is_empty()))
}

/// Waits for `call`: on a terminal with the status row, which goes before the answer
/// is shown; until Ctrl+C everywhere.
async fn wait<T>(
    ctx: &Context,
    out: &mut Output,
    options: &RenderOptions,
    call: impl Future<Output = Result<T, ClientError>>,
) -> Result<T, CliError> {
    let live = options.is_terminal();
    let motion = ctx.settings.render.motion;
    let started = ctx.clock.now();
    let mut interrupt = ctx.interrupt.wait();
    let mut call = std::pin::pin!(call);
    let mut ticks = 0;
    loop {
        if live {
            out.out(&format!(
                "{ERASE_ROW}{}",
                row(motion, ticks, started, ctx.clock.now(), options)
            ))?;
        }
        let tick = ctx.clock.sleep(TICK);
        tokio::select! {
            biased;
            result = &mut call => {
                if live {
                    out.out(ERASE_ROW)?;
                }
                return result.map_err(CliError::from);
            }
            () = &mut interrupt => {
                if live {
                    out.out(ERASE_ROW)?;
                }
                out.err(&format!("{}\n", format::paint(STOPPED, Tone::Dim, options)));
                return Err(CliError::Interrupted);
            }
            () = tick, if live => ticks += 1,
        }
    }
}

/// The status row at tick `ticks`, without its newline: the spinner, the words and,
/// from 1 s on, the time since `started`, cut to the width.
fn row(
    motion: bool,
    ticks: usize,
    started: Timestamp,
    now: Timestamp,
    options: &RenderOptions,
) -> String {
    let elapsed = since_then(started, now);
    let time = (elapsed >= SHOW_TIME).then(|| format::elapsed(elapsed));
    let columns = usize::from(options.width());
    let room = columns.saturating_sub(2 + time.as_ref().map_or(0, |time| time.len() + 2));
    let mut row = options.paint(Role::Accent, &spinner(motion, ticks).to_string());
    let words = format::cut(WORDS, room, options.width_method());
    if !words.is_empty() {
        row.push(' ');
        row.push_str(&options.paint(Role::Muted, &words));
    }
    if let Some(time) = time {
        row.push_str("  ");
        row.push_str(&options.paint(Role::Muted, &time));
    }
    row
}

#[cfg(test)]
mod tests;
