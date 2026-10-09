//! `efr login openai`: relay the daemon's subscription login. `efr login openai-api`
//! and `efr login anthropic`: hand an API key to the daemon.
//!
//! The daemon runs the whole subscription login (PKCE, the loopback listener, the
//! token exchange, storing the credentials); the CLI never touches them. It prints the
//! authorize URL that the daemon streams first, opens it in a browser only when
//! `EFR_OPEN_BROWSER` is on, and waits until the daemon reports the login complete.
//! Ctrl+C drops the connection, and the daemon abandons the login.
//!
//! An API key comes from a variable, a hidden prompt or stdin (`key.rs`) and goes to
//! `admin.login_api_key` as `SecretText`; the daemon checks it unless `--no-check` says
//! not to, and stores it. The CLI shows only the hint of the key that the daemon
//! answers. A login never changes the provider of new conversations: when the key is
//! not for it, the CLI says how to change `[model] provider` and that efrd needs a
//! restart. After a login to `openai-api`, one line says that OpenAI bills the key per
//! token, apart from a ChatGPT plan.

mod key;

use efr_client::ClientError;
use efr_protocol::{
    AdminLoginApiKey, AdminLoginApiKeyResult, AdminLoginOpenAi, AdminLoginOpenAiItem, ErrorCode,
    Method, Origin,
};
use futures::StreamExt as _;
use serde_json::Value;

pub(crate) use self::key::KeyProvider;
use crate::cli::{KeyArgs, LoginCommand};
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

/// How efrd restarts as a user service, after a change of `[model] provider`.
const RESTART: &str = "systemctl --user restart efrd";

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &LoginCommand,
) -> Result<(), CliError> {
    match command {
        LoginCommand::Openai => openai(ctx, out).await,
        LoginCommand::OpenaiApi(args) => api_key(ctx, out, KeyProvider::OpenAi, args).await,
        LoginCommand::Anthropic(args) => api_key(ctx, out, KeyProvider::Anthropic, args).await,
    }
}

/// Reads the key, hands it to the daemon and says what the daemon stored.
async fn api_key(
    ctx: &Context,
    out: &mut Output,
    provider: KeyProvider,
    args: &KeyArgs,
) -> Result<(), CliError> {
    let key = key::read(ctx, out, provider, args.from_env).await?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let check = !args.no_check;
    if check {
        out.err("checking the key...\n");
    }
    let method = Method::AdminLoginApiKey(AdminLoginApiKey {
        provider: provider.id().to_owned(),
        key,
        check,
    });
    let result: AdminLoginApiKeyResult =
        client.call(method).await.map_err(|error| match error {
            // NOTE: a refused or invalid key must not be stored without a check; a check
            // that got no answer, or a busy one, may be skipped.
            ClientError::Server { body }
                if check && !matches!(body.code, ErrorCode::Unauthorized | ErrorCode::Invalid) =>
            {
                CliError::KeyNotChecked { body, retry: provider.without_check() }
            }
            other => other.into(),
        })?;
    let checked = if result.checked { "" } else { " (not checked)" };
    out.out(&format!(
        "logged in to {} with key {}{checked}\n",
        format::one_line(&result.provider),
        format::one_line(&result.key_hint)
    ))?;
    if let Some(billing) = provider.billing() {
        out.out(billing)?;
    }
    if !result.active {
        out.out(&switch(provider, ctx.settings.provider()))?;
    }
    Ok(())
}

/// How to make `provider` the one of new conversations, when the config names
/// `configured`.
fn switch(provider: KeyProvider, configured: &str) -> String {
    let id = provider.id();
    if configured == id {
        return format!(
            "config.toml names {id} under [model], and efrd uses it after a restart: {RESTART}\n"
        );
    }
    format!(
        "New conversations keep their provider. To use {id}, set provider = \"{id}\" under \
         [model] in config.toml (efr config set model.provider {id}), then restart efrd: \
         {RESTART}\n"
    )
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
