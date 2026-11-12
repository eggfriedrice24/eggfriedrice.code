//! `efr status`: whether the daemon runs, and its health (TM-Commands frame 1).
//!
//! The daemon's answer to `admin.status` names where the model list of the account of
//! new conversations came from; the answer to `models.list` names it for each account
//! with a login, so the `models` row asks for it too. A daemon that cannot answer it
//! leaves the row with the one list.

use efr_protocol::{AdminStatus, AdminStatusResult, Method, ModelsList, ModelsListResult, Origin};

use crate::context::Context;
use crate::error::{CliError, login_hint};
use crate::format::{self, StatusView};
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let socket = ctx.socket().await?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await?;
    let catalogs = match client.call::<ModelsListResult>(Method::ModelsList(ModelsList {})).await {
        Ok(list) => list.catalogs,
        Err(error) => {
            tracing::debug!(error = %error, "the model lists of the accounts could not be read");
            Vec::new()
        }
    };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let view = StatusView {
        status: &status,
        catalogs: &catalogs,
        socket: &socket,
        now: ctx.clock.now(),
        file_mode: ctx.settings.turn.mode,
        home: ctx.home.as_deref(),
    };
    out.out(&format::status(&view, &options))?;
    // Every prompt fails until the provider of new conversations has credentials
    // that it takes; stdout stays the daemon's answer alone. An earlier daemon names no
    // active provider, so then only no login at all is worth the line.
    match status.providers.iter().find(|provider| provider.active) {
        Some(active) if !active.logged_in => {
            let provider = format::one_line(&active.provider);
            out.err(&format!(
                "efr: {provider} is not logged in; {}\n",
                login_hint(&active.provider)
            ));
        }
        Some(active) if active.key_refused_at.is_some() => {
            let provider = format::one_line(&active.provider);
            out.err(&format!(
                "efr: the provider refused the key of {provider}; its turns fail until a request \
                 with the key works or a new login; {}\n",
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
