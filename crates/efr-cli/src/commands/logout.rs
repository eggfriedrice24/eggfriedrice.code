//! `efr logout <provider>`: the daemon deletes the stored login of a provider.
//!
//! An API key stays valid at its provider after a logout; the CLI says where its owner
//! revokes it.

use efr_protocol::{AdminLogout, AdminLogoutResult, Method, Origin};

use crate::cli::{LogoutArgs, ProviderName};
use crate::commands::login::KeyProvider;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &LogoutArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let method = Method::AdminLogout(AdminLogout { provider: args.provider.id().to_owned() });
    let result: AdminLogoutResult = client.call(method).await?;
    let provider = format::one_line(&result.provider);
    if !result.logged_out {
        out.out(&format!("{provider} was not logged in\n"))?;
        return Ok(());
    }
    let key = match args.provider {
        ProviderName::OpenAiApi => Some(KeyProvider::OpenAi),
        ProviderName::Anthropic => Some(KeyProvider::Anthropic),
        ProviderName::OpenAi => None,
    };
    match key {
        Some(key) => out.out(&format!(
            "logged out of {provider}. The key stays valid at {}; {}.\n",
            key.company(),
            key.revoke()
        )),
        None => out.out(&format!("logged out of {provider}\n")),
    }
}

#[cfg(test)]
mod tests;
