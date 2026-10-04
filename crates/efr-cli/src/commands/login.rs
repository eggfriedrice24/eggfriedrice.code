//! `efr login openai`: relay the daemon's subscription login.
//!
//! The daemon runs the whole login (PKCE, the loopback listener, the token exchange,
//! storing the credentials); the CLI never touches them. It prints the authorize URL
//! that the daemon streams first, opens it in a browser only when `EFR_OPEN_BROWSER`
//! is on, and waits until the daemon reports the login complete. Ctrl+C drops the
//! connection, and the daemon abandons the login.

use efr_protocol::{AdminLoginOpenAi, AdminLoginOpenAiItem, Method, Origin};
use futures::StreamExt as _;
use serde_json::Value;

use crate::cli::LoginCommand;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &LoginCommand,
) -> Result<(), CliError> {
    match command {
        LoginCommand::Openai => openai(ctx, out).await,
    }
}

async fn openai(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let open_browser = ctx.open_browser()?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let mut stream =
        client.stream::<Value>(Method::AdminLoginOpenAi(AdminLoginOpenAi::default())).await?;
    let mut interrupt = ctx.interrupt.wait();
    let mut completed = false;
    loop {
        let item = tokio::select! {
            () = &mut interrupt => return Err(CliError::Interrupted),
            item = stream.next() => item,
        };
        let value = match item {
            Some(Ok(value)) => value,
            Some(Err(error)) => return Err(error.into()),
            None if completed => return Ok(()),
            None => return Err(CliError::LoginIncomplete),
        };
        match serde_json::from_value::<AdminLoginOpenAiItem>(value) {
            Ok(AdminLoginOpenAiItem::AuthorizeUrl { url }) => {
                out.out(&format!(
                    "Open this URL in a browser to log in to OpenAI:\n\n  {}\n\n",
                    format::one_line(&url)
                ))?;
                if open_browser && let Err(error) = ctx.browser.open(&url) {
                    out.err(&format!("efr: the browser could not be opened: {error}\n"));
                }
                out.out("Waiting for the browser to finish the login...\n")?;
            }
            Ok(AdminLoginOpenAiItem::Completed { provider }) => {
                completed = true;
                out.out(&format!("Logged in to {}.\n", format::one_line(&provider)))?;
            }
            Ok(_) | Err(_) => {
                tracing::debug!("skipped a login item of a kind this build does not know");
            }
        }
    }
}

#[cfg(test)]
mod tests;
