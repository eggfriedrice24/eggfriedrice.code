//! `efr context [conversation]`: what fills the model's context of a conversation, and
//! the cache use of its model calls (`conversation.context`). The zsh plugin runs it for
//! `,context`.
//!
//! Without a conversation it shows this terminal's conversation (the newest whose `tty`
//! is the terminal on stdin), else the newest of all, and a muted line on stderr says
//! which. The lines are in `format/context_report.rs`: the provider's last count first,
//! then the estimate of the next request by part, its largest items, the compactions
//! and the cache. `--json` prints the daemon's answer as it came, for scripts. Neither
//! form holds the text of a message. `--model` counts for another model: its window,
//! and whether the provider items of earlier turns go back.

use efr_protocol::{ConversationContext, ConversationContextResult, Method, Origin};
use serde_json::Value;

use crate::cli::ContextArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, context_report};
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &ContextArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let (conversation_id, picked) = match &args.conversation {
        Some(query) => (super::history::resolve(&client, query).await?, None),
        None => match super::history::pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => (id, Some(picked)),
            None => return out.out("no conversations yet\n"),
        },
    };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    if let Some(picked) = picked {
        let line = format!("{}; efr context <id> shows another", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
    let model = args.model.clone().filter(|model| !model.trim().is_empty());
    let method = Method::ConversationContext(ConversationContext { conversation_id, model });
    if args.json {
        // NOTE: the answer as it came, so a member that a newer efrd adds is kept.
        let answer: Value = client.call(method).await?;
        let mut text = serde_json::to_string_pretty(&answer)
            .map_err(|error| CliError::Output { source: error.into() })?;
        text.push('\n');
        return out.out(&text);
    }
    let result: ConversationContextResult = client.call(method).await?;
    out.out(&context_report::lines(&result, ctx.clock.now(), &options))
}

#[cfg(test)]
mod tests;
