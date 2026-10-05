//! `efr new`: start a new conversation for the terminal with its first prompt, and
//! follow the reply.
//!
//! A conversation exists only once its first prompt is recorded, so `efr new` needs
//! one. The zsh plugin's bare `,new` therefore sends nothing; it makes the next `,`
//! line run `efr new` instead of `efr send`.

use crate::cli::NewArgs;
use crate::commands::send::{self, Handover, Prompt};
use crate::context::Context;
use crate::error::CliError;
use crate::output::Output;
use crate::turn_settings::Asked;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &NewArgs) -> Result<(), CliError> {
    let handover = Handover::read(
        ctx,
        args.context_json.as_deref(),
        args.last_command.as_ref(),
        &args.prompt,
    )?;
    let Handover { context, origin, last_command, text } = handover;
    if text.trim().is_empty() {
        return Err(CliError::NewWithoutPrompt);
    }
    let prompt = Prompt {
        conversation: None,
        new_conversation: true,
        text,
        context,
        last_command,
        settings: Asked::read(ctx, &args.settings)?.to_wire(),
    };
    send::send(ctx, out, origin, prompt).await
}

#[cfg(test)]
mod tests;
