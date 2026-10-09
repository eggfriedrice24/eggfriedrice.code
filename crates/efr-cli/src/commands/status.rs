//! `efr status`: whether the daemon runs, and its health.

use efr_protocol::{AdminStatus, AdminStatusResult, Method, Origin};

use crate::context::Context;
use crate::error::{CliError, login_hint};
use crate::format;
use crate::output::Output;

pub(crate) async fn run(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let socket = ctx.socket().await?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await?;
    out.out(&format::status(&status, &socket, ctx.clock.now()))?;
    // Every prompt fails until the provider of new conversations has credentials;
    // stdout stays the daemon's answer alone. An earlier daemon names no active
    // provider, so then only no login at all is worth the line.
    match status.providers.iter().find(|provider| provider.active) {
        Some(active) if !active.logged_in => {
            let provider = format::one_line(&active.provider);
            out.err(&format!(
                "efr: {provider} is not logged in; {}\n",
                login_hint(&active.provider)
            ));
        }
        Some(_) => {}
        None if !status.providers.is_empty()
            && status.providers.iter().all(|provider| !provider.logged_in) =>
        {
            let hint = login_hint(ctx.settings.provider());
            out.err(&format!("efr: no model provider is logged in; {hint}\n"));
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
