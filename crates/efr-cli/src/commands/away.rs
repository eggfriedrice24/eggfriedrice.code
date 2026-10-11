//! `efr away`: tell efr that the user is away from a conversation (`presence.set`). The
//! zsh plugin runs it for `,away`.
//!
//! While the user is away, the reviewer of `auto` still decides most actions, but a
//! question for the user waits only `presence.away_wait_secs`; then its call does not
//! run, and the model goes on without it and names it in its answer. The next input of
//! the user in the conversation (a prompt, a steer, an answer, keys for the hidden
//! shell) ends it, so the command needs no way back.
//!
//! Without `--conversation`, the command sets this terminal's conversation (the newest
//! whose `tty` is the terminal on stdin), else the newest of all, and a muted line on
//! stderr says which. Then one muted line says what changed. The wait comes from the
//! `config.toml` that `efr` reads, the file that efrd reads too, unless one of them
//! names another.

use std::time::Duration;

use efr_protocol::{Method, Origin, Presence, PresenceSet, PresenceSetResult};

use crate::cli::AwayArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone};
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &AwayArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let (conversation_id, picked) = match &args.conversation {
        Some(query) => (super::history::resolve(&client, query).await?, None),
        None => match super::history::pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => (id, Some(picked)),
            None => return Err(CliError::NobodyToBeAwayFrom),
        },
    };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    if let Some(picked) = picked {
        let line = format!("{}; efr away --conversation <id> sets another", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
    let method = Method::PresenceSet(PresenceSet {
        command_id: ctx.command_id(),
        conversation_id: Some(conversation_id),
        presence: Presence::Away,
    });
    let result: PresenceSetResult = client.call(method).await?;
    let wait = Duration::from_secs(u64::from(ctx.settings.presence.away_wait_secs));
    let line = away_line(!result.changed.is_empty(), wait);
    out.out(&format!("{}\n", format::paint(&line, Tone::Dim, &options)))
}

/// The line after `efr away`: `changed` when the conversation was present before, and
/// how long a question waits now.
pub(crate) fn away_line(changed: bool, wait: Duration) -> String {
    let state = if changed { "away" } else { "away already" };
    format!(
        "{state}: a question now waits {}, then its call does not run; your next input here \
         ends this",
        wait_words(wait)
    )
}

/// A wait in words: whole minutes as `5 minutes`, else as the status row counts time.
fn wait_words(wait: Duration) -> String {
    let seconds = wait.as_secs();
    match seconds {
        60 => "1 minute".to_owned(),
        _ if seconds > 0 && seconds.is_multiple_of(60) => format!("{} minutes", seconds / 60),
        _ => format::elapsed(wait),
    }
}

#[cfg(test)]
mod tests;
