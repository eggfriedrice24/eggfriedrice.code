//! `efr push test` (`admin.push_test`): a test notice to each phone that has push on,
//! through the Expo push service, and one line for each phone: its name, then what
//! became of the notice (TM-Commands, the 0.0.13 frames). efrd answers after the
//! receipts, in about 14 seconds.

use efr_protocol::{
    AdminPushTest, AdminPushTestResult, Method, Origin, PUSH_DEVICE_NOT_REGISTERED,
    PUSH_INVALID_CREDENTIALS, PUSH_RATE_LIMITED, PUSH_UNAUTHORIZED, PushTestResult,
};
use efr_render::RenderOptions;
use unicode_width::UnicodeWidthStr as _;

use crate::cli::PushCommand;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone};
use crate::live::effective_width;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &PushCommand,
) -> Result<(), CliError> {
    match command {
        PushCommand::Test => test(ctx, out).await,
    }
}

async fn test(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let tested: AdminPushTestResult = client.call(Method::AdminPushTest(AdminPushTest {})).await?;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    out.out(&lines(&tested, &options))
}

/// One line for each phone: its name, padded to the longest, then the result.
pub(crate) fn lines(tested: &AdminPushTestResult, options: &RenderOptions) -> String {
    if tested.devices.is_empty() {
        return format::own_line(&[("no paired phone has push on".to_owned(), Tone::Dim)], options);
    }
    let names: Vec<String> = tested.devices.iter().map(|row| format::one_line(&row.name)).collect();
    let width = names.iter().map(|name| name.width()).max().unwrap_or(0);
    let mut text = String::new();
    for (row, name) in tested.devices.iter().zip(names) {
        let pad = width - name.width();
        let (result, tone) = result(row.result, row.code.as_deref());
        let spans = [(format!("{name}{:pad$}  ", ""), Tone::Plain), (result, tone)];
        text.push_str(&format::own_line(&spans, options));
    }
    text
}

/// What became of one notice, in words, with its tone.
fn result(result: PushTestResult, code: Option<&str>) -> (String, Tone) {
    match result {
        PushTestResult::Sent => ("sent".to_owned(), Tone::Success),
        PushTestResult::NoReceipt => ("sent to Expo, no receipt yet".to_owned(), Tone::Attention),
        PushTestResult::NotSent => (not_sent(code), Tone::Failure),
        _ => ("no result that this efr knows".to_owned(), Tone::Dim),
    }
}

/// Why a notice did not go out, by the code of the Expo push service, and what to do.
fn not_sent(code: Option<&str>) -> String {
    let Some(code) = code.map(format::one_line) else {
        return "not sent: no answer from exp.host.".to_owned();
    };
    match code.as_str() {
        PUSH_DEVICE_NOT_REGISTERED => {
            format!("not sent: {code}. Push is off for this phone now. Open the app.")
        }
        PUSH_INVALID_CREDENTIALS => {
            format!(
                "not sent: {code}. Expo has no valid push key for this app. Run eas credentials."
            )
        }
        PUSH_UNAUTHORIZED => {
            format!("not sent: {code}. The Expo account asks for an access token. efr sends none.")
        }
        rate if PUSH_RATE_LIMITED.contains(&rate) => format!("not sent: {code}. Try again later."),
        _ => format!("not sent: {code}."),
    }
}

#[cfg(test)]
mod tests;
