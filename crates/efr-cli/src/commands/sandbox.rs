//! `efr sandbox check`, `efr sandbox explain` and `efr sandbox allow`: the sandbox of the
//! `auto` mode, as the daemon sees it, and the always-allow rules.
//!
//! `check` asks the daemon to run its probe now (`admin.sandbox_check`), prints every
//! check with its outcome and the fix of a failed one, and exits with 1 when the
//! sandbox is not available, so a script can test it. `explain` asks what a contained
//! command can do with one path (`sandbox.explain`), from the current directory, which
//! picks the project. `allow` lists the always-allow rules of `config.toml`, and
//! `allow remove N` removes one after a question.

mod allow;

use std::path::Path;

use efr_protocol::{
    AdminSandboxCheck, AdminSandboxCheckResult, Method, Origin, SandboxExplain,
    SandboxExplainResult,
};

use crate::cli::{AllowCommand, SandboxCommand};
use crate::context::Context;
use crate::error::CliError;
use crate::format::sandbox;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &SandboxCommand,
) -> Result<(), CliError> {
    match command {
        SandboxCommand::Check => check(ctx, out).await,
        SandboxCommand::Explain { path } => explain(ctx, out, path).await,
        SandboxCommand::Allow { command: None } => allow::list(ctx, out).await,
        SandboxCommand::Allow { command: Some(AllowCommand::Remove { index }) } => {
            allow::remove(ctx, out, *index).await
        }
    }
}

async fn check(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let method = Method::AdminSandboxCheck(AdminSandboxCheck {});
    let result: AdminSandboxCheckResult = client.call(method).await?;
    out.out(&sandbox::check(&result))?;
    if result.status.available { Ok(()) } else { Err(CliError::SandboxUnavailable) }
}

async fn explain(ctx: &Context, out: &mut Output, path: &Path) -> Result<(), CliError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.as_ref().ok_or(CliError::NoWorkingDirectory)?.join(path)
    };
    let client = ctx.connect(Origin::Cli, None).await?;
    let method = Method::SandboxExplain(SandboxExplain { path, cwd: ctx.cwd.clone() });
    let result: SandboxExplainResult = client.call(method).await?;
    out.out(&sandbox::explain(&result, ctx.home.as_deref()))
}

#[cfg(test)]
mod tests;
