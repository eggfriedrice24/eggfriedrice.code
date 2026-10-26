//! `efr device add`, `list` and `revoke`: pair a phone with efrd over Tailscale, and
//! manage the paired devices. Each call goes through the Unix socket, where only the
//! user's own process holds the `admin` scope.
//!
//! - `add` (`device.pair_start`) prints the QR code inline, below the command line, as
//!   text in half blocks (`qr.rs`), then when the token expires. When a phone sends the
//!   token, it prints the phone's facts (its name, its Tailscale node and user, the
//!   fingerprint of its key) and asks on one line: `y` pairs it, anything else refuses
//!   it (`device.pair_confirm`). It needs a terminal for that question; in a pipe it
//!   stops before it opens a pairing. Ctrl+C ends the pairing, and its token is then
//!   good for nothing. A no prints `nothing was paired` and exits 0; an expiry, too many
//!   wrong tokens or a phone that left exit 1.
//! - `list` (`device.list`) prints one row per device: the start of its id, its name,
//!   its Tailscale node, when it was paired, when it was last seen and whether it is
//!   revoked. `--json` prints the daemon's answer.
//! - `revoke ID` (`device.revoke`) takes a whole id, or the start of exactly one id of
//!   the list, and prints when the device was revoked.

mod qr;

use std::fmt::Write as _;

use efr_protocol::{
    DeviceId, DeviceInfo, DeviceList, DeviceListResult, DevicePairConfirm, DevicePairConfirmResult,
    DevicePairStart, DevicePairStartItem, DeviceRevoke, DeviceRevokeResult, Method, Origin,
    PairEnd, PairingId,
};
use efr_render::RenderOptions;
use efr_transport::client::{Client, ClientError, ItemStream};
use futures::StreamExt as _;
use jiff::Timestamp;
use serde_json::Value;
use unicode_width::UnicodeWidthStr as _;

use crate::cli::{DeviceAddArgs, DeviceCommand, DeviceListArgs, DeviceRevokeArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone};
use crate::keys::Key;
use crate::live::effective_width;
use crate::output::Output;

/// The characters of a device id that a row shows, as for a conversation.
const SHORT_ID: usize = 8;

/// The fewest characters of an id start that `revoke` takes.
const MIN_ID_START: usize = 4;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &DeviceCommand,
) -> Result<(), CliError> {
    match command {
        DeviceCommand::Add(args) => add(ctx, out, args).await,
        DeviceCommand::List(args) => list(ctx, out, args).await,
        DeviceCommand::Revoke(args) => revoke(ctx, out, args).await,
    }
}

async fn add(ctx: &Context, out: &mut Output, args: &DeviceAddArgs) -> Result<(), CliError> {
    if !ctx.keys.available() {
        return Err(CliError::DeviceAddNeedsTerminal);
    }
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let client = ctx.connect(Origin::Cli, None).await?;
    let params = DevicePairStart { name: args.name.clone() };
    let mut stream = client.stream::<Value>(Method::DevicePairStart(params)).await?;
    let mut interrupt = ctx.interrupt.wait();
    let mut refused = false;
    loop {
        let item = tokio::select! {
            () = &mut interrupt => return Err(CliError::Interrupted),
            item = stream.next() => item,
        };
        let item = match decode(item)? {
            Some(item) => item,
            None => continue,
        };
        match item {
            DevicePairStartItem::Started { expires_at, qr, .. } => {
                let lines = qr::lines(qr.url().expose_secret(), args.invert)
                    .map_err(|reason| CliError::PairingCode { reason })?;
                out.out(&started(ctx, &lines, expires_at, &options))?;
            }
            DevicePairStartItem::Pending {
                pairing_id,
                device_name,
                node_name,
                user_login,
                key_fingerprint,
                os,
            } => {
                let facts = Facts {
                    given_name: args.name.as_deref(),
                    device_name: &device_name,
                    node_name: &node_name,
                    user_login: &user_login,
                    key_fingerprint: &key_fingerprint,
                    os: os.as_deref(),
                };
                out.out(&pending(&facts, &options))?;
                loop {
                    match ask(ctx, &mut stream).await? {
                        Asked::Answer(accept) => {
                            confirm(&client, pairing_id, accept).await?;
                            refused = !accept;
                            break;
                        }
                        // NOTE: the pairing ended while the question was open, such as
                        // at its expiry; the item says how.
                        Asked::Item(item) => {
                            if let Some(item) = decode(item)? {
                                return ended(out, item, refused, &options);
                            }
                        }
                    }
                }
            }
            other => return ended(out, other, refused, &options),
        }
    }
}

/// The item of `item`, `None` for an item of a kind that this build does not know.
fn decode(
    item: Option<Result<Value, ClientError>>,
) -> Result<Option<DevicePairStartItem>, CliError> {
    match item {
        Some(Ok(value)) => match serde_json::from_value::<DevicePairStartItem>(value) {
            Ok(item) => Ok(Some(item)),
            Err(_) => {
                tracing::debug!("skipped a pairing item of a kind this build does not know");
                Ok(None)
            }
        },
        Some(Err(error)) => Err(error.into()),
        None => Err(CliError::PairingIncomplete),
    }
}

/// The last item of a pairing: the device, or why there is none.
fn ended(
    out: &mut Output,
    item: DevicePairStartItem,
    refused: bool,
    options: &RenderOptions,
) -> Result<(), CliError> {
    match item {
        DevicePairStartItem::Paired { device_id, .. } => {
            let short = short_id(device_id);
            let line = format!(
                "paired the device {short}; efr device revoke {short} takes its access away"
            );
            out.out(&format!("{}\n", format::paint(&line, Tone::Success, options)))
        }
        DevicePairStartItem::Ended { reason: PairEnd::Refused, .. } if refused => {
            out.err(&format!("{}\n", format::paint("nothing was paired", Tone::Dim, options)));
            Ok(())
        }
        DevicePairStartItem::Ended { reason, .. } => Err(CliError::PairingEnded { reason }),
        _ => Err(CliError::PairingIncomplete),
    }
}

/// The QR code and what to do with it.
fn started(
    ctx: &Context,
    lines: &[String],
    expires_at: Timestamp,
    options: &RenderOptions,
) -> String {
    let mut text = String::from("Scan this code with the efr app on your phone:\n\n");
    for line in lines {
        let _ = writeln!(text, "{line}");
    }
    let columns = lines.first().map_or(0, |line| line.width());
    let window = usize::from(ctx.screen.size().cols);
    if window > 0 && window < columns {
        let note = format!("the window is narrower than the code; make it {columns} columns wide");
        let _ = writeln!(text, "{}", format::paint(&note, Tone::Attention, options));
    }
    let until = format!(
        "The code is good for one phone until {} ({}). Ctrl+C stops the pairing.",
        format::local_time(expires_at, &ctx.time_zone),
        format::until(expires_at, ctx.clock.now()),
    );
    let _ = writeln!(text, "\n{}", format::paint(&until, Tone::Dim, options));
    text
}

/// What the terminal shows of a phone that sent the token.
struct Facts<'a> {
    given_name: Option<&'a str>,
    device_name: &'a str,
    node_name: &'a str,
    user_login: &'a str,
    key_fingerprint: &'a str,
    os: Option<&'a str>,
}

/// The phone's facts and the question.
fn pending(facts: &Facts<'_>, options: &RenderOptions) -> String {
    let name = match facts.given_name {
        Some(given) if given != facts.device_name => format!(
            "{} (the phone calls itself {})",
            format::one_line(given),
            format::one_line(facts.device_name)
        ),
        _ => format::one_line(facts.device_name),
    };
    let mut rows = vec![
        ("name", name),
        (
            "tailscale",
            format!(
                "{} ({})",
                format::one_line(facts.node_name),
                format::one_line(facts.user_login)
            ),
        ),
    ];
    if let Some(os) = facts.os {
        rows.push(("system", format::one_line(os)));
    }
    rows.push(("key", format::one_line(facts.key_fingerprint)));
    let mut text = String::from("\nA phone wants to pair:\n");
    for (label, value) in rows {
        let label = format::paint(&format!("{label:<10}"), Tone::Dim, options);
        let _ = writeln!(text, "  {label}{value}");
    }
    let question = "Compare the key with the one that the app shows. Pair this phone? [y/N]";
    let _ = writeln!(text, "{}", format::paint(question, Tone::Attention, options));
    text
}

/// What came first while efr asked: the user's answer, or the next item of the
/// pairing.
enum Asked {
    Answer(bool),
    Item(Option<Result<Value, ClientError>>),
}

/// Reads the answer: `y` is yes; `n`, Enter, Esc and any end of the keys are no. The
/// pairing's next item ends the question, because then there is nothing to answer.
async fn ask(ctx: &Context, stream: &mut ItemStream<Value>) -> Result<Asked, CliError> {
    let mut reader = ctx.keys.start()?;
    let mut interrupt = ctx.interrupt.wait();
    let asked = loop {
        tokio::select! {
            key = reader.next() => match key {
                Some(Key::Byte(b'y' | b'Y')) => break Ok(Asked::Answer(true)),
                Some(Key::Byte(b'n' | b'N' | b'\r' | b'\n')) | Some(Key::Esc) | None => {
                    break Ok(Asked::Answer(false));
                }
                Some(Key::Byte(_)) => {}
            },
            item = stream.next() => break Ok(Asked::Item(item)),
            () = &mut interrupt => break Err(CliError::Interrupted),
        }
    };
    reader.stop().await;
    asked
}

async fn confirm(client: &Client, pairing_id: PairingId, accept: bool) -> Result<(), CliError> {
    let params = DevicePairConfirm { pairing_id, accept };
    let _: DevicePairConfirmResult = client.call(Method::DevicePairConfirm(params)).await?;
    Ok(())
}

async fn list(ctx: &Context, out: &mut Output, args: &DeviceListArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let method = Method::DeviceList(DeviceList {});
    if args.json {
        // NOTE: the answer as it came, so a member that a newer efrd adds is kept.
        let answer: Value = client.call(method).await?;
        let mut text = serde_json::to_string_pretty(&answer)
            .map_err(|error| CliError::Output { source: error.into() })?;
        text.push('\n');
        return out.out(&text);
    }
    let listed: DeviceListResult = client.call(method).await?;
    out.out(&listing(&listed, ctx.clock.now()))
}

/// One row per device: the start of its id, its name, its node, when it was paired and
/// last seen, and `revoked` with its age for a revoked one.
pub(crate) fn listing(listed: &DeviceListResult, now: Timestamp) -> String {
    if listed.devices.is_empty() {
        return "no device is paired; efr device add pairs a phone\n".to_owned();
    }
    let names: Vec<String> =
        listed.devices.iter().map(|device| format::one_line(&device.name)).collect();
    let width = names.iter().map(|name| name.width()).max().unwrap_or(0);
    let mut out = String::new();
    for (device, name) in listed.devices.iter().zip(&names) {
        let pad = width - name.width();
        let _ = writeln!(
            out,
            "{}  {name}{:pad$}  {}  {}",
            short_id(device.device_id),
            "",
            format::one_line(&device.node),
            state(device, now)
        );
    }
    out
}

/// When a device was paired and last seen, or when it was revoked.
fn state(device: &DeviceInfo, now: Timestamp) -> String {
    let paired = format!("paired {}", format::ago(device.created_at, now));
    match (device.revoked_at, device.last_seen_at) {
        (Some(revoked), _) => format!("{paired}, revoked {}", format::ago(revoked, now)),
        (None, Some(seen)) => format!("{paired}, seen {}", format::ago(seen, now)),
        (None, None) => format!("{paired}, never connected"),
    }
}

async fn revoke(ctx: &Context, out: &mut Output, args: &DeviceRevokeArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let (device_id, name) = find(&client, &args.id).await?;
    let revoked: DeviceRevokeResult =
        client.call(Method::DeviceRevoke(DeviceRevoke { device_id })).await?;
    let what = match name {
        Some(name) => format!("{} ({})", format::one_line(&name), short_id(device_id)),
        None => short_id(device_id),
    };
    out.out(&format!(
        "revoked {what} at {}; it can no longer connect\n",
        format::local_time(revoked.revoked_at, &ctx.time_zone)
    ))
}

/// The device that `query` names: a whole id, or the start of exactly one id of the
/// list, with its name when the list has it.
async fn find(client: &Client, query: &str) -> Result<(DeviceId, Option<String>), CliError> {
    let listed: DeviceListResult = client.call(Method::DeviceList(DeviceList {})).await?;
    let query_lower = query.to_ascii_lowercase();
    if let Ok(device_id) = query.parse::<DeviceId>() {
        let name = listed.devices.iter().find(|d| d.device_id == device_id).map(|d| d.name.clone());
        return Ok((device_id, name));
    }
    if query.chars().count() < MIN_ID_START {
        return Err(CliError::DeviceNotFound { query: query.to_owned() });
    }
    let matches: Vec<&DeviceInfo> = listed
        .devices
        .iter()
        .filter(|device| device.device_id.to_string().starts_with(&query_lower))
        .collect();
    match matches.as_slice() {
        [device] => Ok((device.device_id, Some(device.name.clone()))),
        [] => Err(CliError::DeviceNotFound { query: query.to_owned() }),
        more => Err(CliError::AmbiguousDevice { query: query.to_owned(), matches: more.len() }),
    }
}

/// The first characters of `id`, as `list` shows them.
fn short_id(id: DeviceId) -> String {
    id.to_string().chars().take(SHORT_ID).collect()
}

#[cfg(test)]
mod tests;
