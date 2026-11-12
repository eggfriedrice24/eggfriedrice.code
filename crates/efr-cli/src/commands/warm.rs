//! `efr warm`: make the connection of this terminal's next prompt ready
//! (`provider.warm`). The zsh plugin starts it in the background, once for each prompt
//! line, while the user types it; it is hidden from the help.
//!
//! It sends the terminal, the working directory and the turn settings that the plugin
//! hands over (`EFR_MODEL`, which can name a provider as `provider/model`), never any
//! text of the prompt. efrd answers at once and warms on its own, so the command ends
//! after one round trip. It writes nothing: a failure, such as a daemon that does not
//! run, only sets the exit code.

use efr_protocol::{Method, Origin, ProviderWarm, ProviderWarmResult};

use crate::cli::{TurnSettingsArgs, WarmArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::turn_settings::Asked;

pub(crate) async fn run(ctx: &Context, args: &WarmArgs) -> Result<(), CliError> {
    let settings = Asked::read(ctx, &TurnSettingsArgs::default())?.to_wire();
    let tty = args.tty.clone().or_else(|| ctx.tty.clone());
    let client = ctx.connect(Origin::Cli, tty.as_deref()).await?;
    let params = ProviderWarm {
        tty,
        pwd: ctx.cwd.clone(),
        model: settings.model,
        provider: settings.provider,
        new_conversation: args.new,
    };
    let _: ProviderWarmResult = client.call(Method::ProviderWarm(params)).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
