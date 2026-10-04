//! `efr status`: whether the daemon runs, and its health.

use efr_protocol::{AdminStatus, AdminStatusResult, Method, Origin};

use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

pub(crate) async fn run(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let socket = ctx.socket().await?;
    let client = ctx.connect(Origin::Cli, None).await?;
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await?;
    out.out(&format::status(&status, &socket, ctx.clock.now()))
}

#[cfg(test)]
mod tests;
