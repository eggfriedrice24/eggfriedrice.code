//! `efr compact [focus]`: compact the context of this terminal's conversation now
//! (`conversation.compact`). The zsh plugin's `,compact` runs it with the focus in
//! `EFR_PROMPT`.
//!
//! The daemon prunes old tool output and writes a summary of the history before the
//! verbatim tail, with the focus words as what the summary must keep. It never starts
//! a turn. `efr` waits for the answer and prints the compaction's muted line, such as
//! `context compacted (efr compact): 140k -> 19.0k tokens, kept 1 turn, summary 2.1k`.
//! While a turn runs, or when nothing lies before the tail, the daemon refuses, and the
//! refusal is the error.

use efr_protocol::{ConversationCompact, ConversationCompactResult, Method, Origin};
use efr_stdx::env::Var;

use crate::cli::CompactArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone};
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &CompactArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, ctx.tty.as_deref()).await?;
    let conversation_id = match (&args.conversation, ctx.tty.as_deref()) {
        (Some(query), _) => super::history::resolve(&client, query).await?,
        (None, Some(tty)) => super::send::active_conversation(&client, tty).await?,
        (None, None) => return Err(CliError::CompactNeedsConversation),
    };
    // NOTE: the zsh plugin hands the focus over in EFR_PROMPT, as it does a prompt, so
    // no other user reads it in the command line.
    let focus = if args.focus.is_empty() {
        ctx.env
            .var(Var::Prompt)
            .map_err(|source| CliError::Environment { source })?
            .unwrap_or_default()
    } else {
        args.focus.join(" ")
    };
    let focus = (!focus.trim().is_empty()).then(|| focus.trim().to_owned());
    let command_id = ctx.command_id();
    let method =
        Method::ConversationCompact(ConversationCompact { command_id, conversation_id, focus });
    let result: ConversationCompactResult = client.call(method).await?;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let line = format::compaction::compacted(&result.compaction);
    out.out(&format!("{}\n", format::paint(&line, Tone::Dim, &options)))
}

#[cfg(test)]
mod tests;
