use std::sync::Arc;

use efr_protocol::{
    DeviceId, DeviceInfo, DeviceListResult, DevicePairConfirm, DevicePairConfirmResult,
    DevicePairStart, DevicePairStartItem, DeviceRevoke, DeviceRevokeResult, ErrorBody, ErrorCode,
    Method, PairEnd, PairingId, PairingQr, ScopeName, SecretText,
};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::listing;
use crate::context::Context;
use crate::error::Exit;
use crate::keys::Keys;
use crate::run;
use crate::testing::{ScriptedKeys, TestEnv, TestInterrupt, capture, command, now};

const DEVICE: &str = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const OTHER: &str = "019a9b1c-3d00-7a10-8b20-0000000000d2";
const PAIRING: &str = "019a9b1c-3d00-7a10-8b20-0000000000a1";

fn device_id() -> DeviceId {
    DEVICE.parse().unwrap()
}

fn pairing_id() -> PairingId {
    PAIRING.parse().unwrap()
}

fn started() -> DevicePairStartItem {
    DevicePairStartItem::Started {
        pairing_id: pairing_id(),
        expires_at: now() + SignedDuration::from_mins(5),
        qr: PairingQr {
            daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
            host: "100.64.0.1".parse().unwrap(),
            port: 7419,
            fqdn: Some("box.tail1234.ts.net".to_owned()),
            token: SecretText::new("q0Zx9mYb3Lk2pR7tW1nV5cHd"),
        },
    }
}

fn pending() -> DevicePairStartItem {
    DevicePairStartItem::Pending {
        pairing_id: pairing_id(),
        device_name: "Anna's iPhone".to_owned(),
        node_name: "annas-iphone.tail1234.ts.net".to_owned(),
        user_login: "anna@example.com".to_owned(),
        key_fingerprint: "SHA256:0ed3a6ab957ff6f5".to_owned(),
        os: Some("iOS".to_owned()),
    }
}

fn device(id: &str, name: &str) -> DeviceInfo {
    DeviceInfo {
        device_id: id.parse().unwrap(),
        name: name.to_owned(),
        key_fingerprint: "SHA256:0ed3a6ab957ff6f5".to_owned(),
        node: "nPHONE1CNTRL".to_owned(),
        scopes: vec![ScopeName::Read, ScopeName::Operate, ScopeName::Approve],
        created_at: now() - SignedDuration::from_hours(3),
        last_seen_at: None,
        revoked_at: None,
    }
}

fn on_a_terminal(env: &TestEnv, keys: &Arc<ScriptedKeys>) -> Context {
    Context { keys: Arc::clone(keys) as Arc<dyn Keys>, ..env.context() }
}

/// What a run of `efr device <args>` did, with the confirms that the fake daemon got.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    confirms: Vec<DevicePairConfirm>,
}

/// How the fake daemon goes on after `pending`.
enum After {
    /// It waits for the terminal's answer, then sends `paired` or `ended`.
    Answer,
    /// It ends the pairing with this reason while the question is open.
    Ends(PairEnd),
}

/// Runs `efr device add <args>` against a fake daemon that sends `started` and
/// `pending`, and presses `key` once the question is open.
async fn add_with(args: &[&str], key: Option<u8>, after: After) -> Ran {
    let env = TestEnv::new();
    let fake = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = on_a_terminal(&env, &keys);
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (stream, method) = conn.request().await;
        let Method::DevicePairStart(DevicePairStart { name }) = method else {
            panic!("expected device.pair_start, got {}", method.name());
        };
        assert_eq!(
            name.as_deref(),
            args.iter().position(|a| *a == "--name").map(|at| args[at + 1])
        );
        conn.item(stream, &started()).await;
        conn.item(stream, &pending()).await;
        let mut confirms = Vec::new();
        match after {
            After::Answer => {
                if let Some(key) = key {
                    keys.press(key).await;
                }
                let (id, method) = conn.request().await;
                let Method::DevicePairConfirm(confirm) = method else {
                    panic!("expected device.pair_confirm, got {}", method.name());
                };
                let accept = confirm.accept;
                confirms.push(confirm);
                let device_id = accept.then(device_id);
                conn.reply(id, &DevicePairConfirmResult { device_id }).await;
                let last = match device_id {
                    Some(device_id) => {
                        DevicePairStartItem::Paired { pairing_id: pairing_id(), device_id }
                    }
                    None => DevicePairStartItem::Ended {
                        pairing_id: pairing_id(),
                        reason: PairEnd::Refused,
                    },
                };
                conn.item(stream, &last).await;
            }
            After::Ends(reason) => {
                let _ = keys.starts();
                conn.item(stream, &DevicePairStartItem::Ended { pairing_id: pairing_id(), reason })
                    .await;
            }
        }
        conn.end(stream).await;
        let _ = conn.until_closed().await;
        confirms
    };
    let mut line = vec!["device", "add"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, confirms) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: captured.stdout(), stderr: captured.stderr(), confirms }
}

#[tokio::test]
async fn add_shows_the_code_and_the_phone_and_y_pairs_it() {
    let ran = add_with(&[], Some(b'y'), After::Answer).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [confirm] = ran.confirms.try_into().unwrap();
    assert_eq!((confirm.pairing_id, confirm.accept), (pairing_id(), true));
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(lines[0], "Scan this code with the efr app on your phone:");
    let code: Vec<&str> =
        lines.iter().copied().skip(2).take_while(|line| !line.is_empty()).collect();
    assert!(code.len() > 20, "{code:?}");
    assert!(code.iter().all(|line| line.chars().all(|c| " \u{2580}\u{2584}\u{2588}".contains(c))));
    let rest = lines[2 + code.len()..].join("\n");
    assert_eq!(
        rest,
        "\n\
         The code is good for one phone until 2026-10-04 12:05:00 UTC (in 5m 0s). Ctrl+C stops the pairing.\n\
         \n\
         A phone wants to pair:\n  \
         name      Anna's iPhone\n  \
         tailscale annas-iphone.tail1234.ts.net (anna@example.com)\n  \
         system    iOS\n  \
         key       SHA256:0ed3a6ab957ff6f5\n\
         Compare the key with the one that the app shows. Pair this phone? [y/N]\n\
         paired the device 019a9b1c; efr device revoke 019a9b1c takes its access away"
    );
    assert!(!ran.stdout.contains("q0Zx9mYb3Lk2pR7tW1nV5cHd"), "the token shows only in the code");
}

#[tokio::test]
async fn add_with_a_name_sends_it_and_shows_both_names() {
    let ran = add_with(&["--name", "work phone"], Some(b'y'), After::Answer).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert!(
        ran.stdout.contains("  name      work phone (the phone calls itself Anna's iPhone)\n"),
        "{}",
        ran.stdout
    );
}

#[tokio::test]
async fn n_refuses_the_phone_and_nothing_is_paired() {
    let ran = add_with(&[], Some(b'n'), After::Answer).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [confirm] = ran.confirms.try_into().unwrap();
    assert!(!confirm.accept);
    assert_eq!(ran.stderr, "nothing was paired\n");
    assert!(!ran.stdout.contains("paired the device"));
}

#[tokio::test]
async fn any_other_key_waits_and_enter_refuses() {
    let env = TestEnv::new();
    let fake = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = on_a_terminal(&env, &keys);
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (stream, _) = conn.request().await;
        conn.item(stream, &started()).await;
        conn.item(stream, &pending()).await;
        keys.press(b'x').await;
        keys.press(b'\r').await;
        let (id, method) = conn.request().await;
        let Method::DevicePairConfirm(confirm) = method else { panic!("a confirm") };
        conn.reply(id, &DevicePairConfirmResult { device_id: None }).await;
        let ended =
            DevicePairStartItem::Ended { pairing_id: pairing_id(), reason: PairEnd::Refused };
        conn.item(stream, &ended).await;
        conn.end(stream).await;
        confirm.accept
    };
    let line = command(&["device", "add"]);
    let (exit, accept) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert!(!accept);
}

#[tokio::test]
async fn an_expiry_while_the_question_is_open_ends_the_command() {
    let ran = add_with(&[], None, After::Ends(PairEnd::Expired)).await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(ran.confirms.is_empty());
    assert_eq!(
        ran.stderr,
        "efr: the pairing expired, or the phone left before the answer; nothing was paired\n"
    );
}

#[tokio::test]
async fn too_many_wrong_tokens_end_the_command() {
    let ran = add_with(&[], None, After::Ends(PairEnd::Attempts)).await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert_eq!(
        ran.stderr,
        "efr: efrd closed the pairing after too many wrong tokens; nothing was paired\n"
    );
}

#[tokio::test]
async fn add_without_a_terminal_opens_no_pairing() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();

    let exit = run::run(&command(&["device", "add"]), &ctx, &mut out).await;

    assert_eq!(exit, Exit::Usage);
    assert_eq!(
        captured.stderr(),
        "efr: efr device add needs a terminal to ask about the phone; nothing was paired\n"
    );
}

#[tokio::test]
async fn a_listener_that_is_off_gets_the_fix() {
    let env = TestEnv::new();
    let fake = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = on_a_terminal(&env, &keys);
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (stream, _) = conn.request().await;
        let body = ErrorBody::new(ErrorCode::Conflict, "the tailnet listener is off")
            .with_data(json!({ "remote": "off" }));
        conn.fail(stream, body).await;
    };
    let line = command(&["device", "add"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::DaemonError);
    let stderr = captured.stderr();
    assert!(stderr.contains("the tailnet listener is off"), "{stderr}");
    assert!(
        stderr.ends_with(
            "efr: turn it on with: efr config set remote.enabled true, then restart efrd: systemctl --user restart efrd\n"
        ),
        "{stderr}"
    );
}

#[tokio::test]
async fn ctrl_c_stops_the_pairing() {
    let env = TestEnv::new();
    let fake = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { interrupt: Arc::clone(&interrupt) as _, ..on_a_terminal(&env, &keys) };
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (stream, _) = conn.request().await;
        conn.item(stream, &started()).await;
        interrupt.trigger();
        conn.until_closed().await
    };
    let line = command(&["device", "add"]);
    let (exit, _frames) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Interrupted);
}

#[tokio::test]
async fn list_prints_one_row_per_device() {
    let env = TestEnv::new();
    let fake = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::DeviceList(_)));
        let mut seen = device(OTHER, "iPad");
        seen.last_seen_at = Some(now() - SignedDuration::from_mins(5));
        let devices = vec![device(DEVICE, "Anna's iPhone"), seen];
        conn.reply(id, &DeviceListResult { devices }).await;
    };
    let line = command(&["device", "list"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert_eq!(
        captured.stdout(),
        "019a9b1c  Anna's iPhone  nPHONE1CNTRL  paired 3h 0m ago, never connected\n\
         019a9b1c  iPad           nPHONE1CNTRL  paired 3h 0m ago, seen 5m 0s ago\n"
    );
}

#[test]
fn listing_shows_a_revoke_and_an_empty_list() {
    let mut revoked = device(DEVICE, "old phone\u{1b}[31m");
    revoked.revoked_at = Some(now() - SignedDuration::from_mins(1));

    let text = listing(&DeviceListResult { devices: vec![revoked] }, now());
    let empty = listing(&DeviceListResult { devices: Vec::new() }, now());

    assert!(text.ends_with("paired 3h 0m ago, revoked 1m 0s ago\n"), "{text}");
    assert!(!text.contains('\u{1b}'), "a name never reaches the terminal raw: {text:?}");
    assert_eq!(empty, "no device is paired; efr device add pairs a phone\n");
}

#[tokio::test]
async fn list_json_prints_the_answer() {
    let env = TestEnv::new();
    let fake = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &DeviceListResult { devices: vec![device(DEVICE, "phone")] }).await;
    };
    let line = command(&["device", "list", "--json"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Success);
    let value: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
    assert_eq!(value["devices"][0]["device_id"], DEVICE);
}

/// Runs `efr device revoke <query>` against a fake daemon that lists `devices`.
async fn revoke_with(
    query: &str,
    devices: Vec<DeviceInfo>,
) -> (Exit, String, String, Vec<DeviceId>) {
    let env = TestEnv::new();
    let fake = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let mut revoked = Vec::new();
        while let Some((id, method)) = next_request(&mut conn).await {
            match method {
                Method::DeviceList(_) => {
                    conn.reply(id, &DeviceListResult { devices: devices.clone() }).await;
                }
                Method::DeviceRevoke(DeviceRevoke { device_id }) => {
                    revoked.push(device_id);
                    let at = now() - SignedDuration::from_secs(1);
                    conn.reply(id, &DeviceRevokeResult { revoked_at: at }).await;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        revoked
    };
    let line = command(&["device", "revoke", query]);
    let (exit, revoked) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    (exit, captured.stdout(), captured.stderr(), revoked)
}

async fn next_request(
    conn: &mut crate::testing::Conn,
) -> Option<(efr_protocol::RequestId, Method)> {
    match conn.recv().await? {
        efr_protocol::ClientFrame::Request { id, method } => Some((id, method)),
        efr_protocol::ClientFrame::Cancel { .. } => None,
        _ => None,
    }
}

#[tokio::test]
async fn revoke_takes_the_start_of_an_id() {
    let devices = vec![device(DEVICE, "Anna's iPhone")];

    let (exit, stdout, stderr, revoked) =
        revoke_with("019a9b1c-3d00-7a10-8b20-0000000000d", devices).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    assert_eq!(revoked, vec![device_id()]);
    assert_eq!(
        stdout,
        "revoked Anna's iPhone (019a9b1c) at 2026-10-04 11:59:59 UTC; it can no longer connect\n"
    );
}

#[tokio::test]
async fn revoke_refuses_a_start_that_matches_two_devices_or_none() {
    let devices = vec![device(DEVICE, "one"), device(OTHER, "two")];

    let (exit, _, stderr, revoked) = revoke_with("019a9b1c", devices.clone()).await;
    assert_eq!(exit, Exit::Usage);
    assert!(revoked.is_empty());
    assert_eq!(stderr, "efr: \"019a9b1c\" matches 2 devices; type more of the id\n");

    let (exit, _, stderr, revoked) = revoke_with("ffff", devices.clone()).await;
    assert_eq!(exit, Exit::DaemonError);
    assert!(revoked.is_empty());
    assert_eq!(stderr, "efr: no device matches \"ffff\"; efr device list shows the devices\n");

    let (_, _, _, revoked) = revoke_with("019", devices).await;
    assert!(revoked.is_empty(), "a start of fewer than 4 characters names no device");
}

#[tokio::test]
async fn revoke_sends_a_whole_id_that_the_list_lacks() {
    let (exit, stdout, stderr, revoked) = revoke_with(DEVICE, Vec::new()).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    assert_eq!(revoked, vec![device_id()]);
    assert!(stdout.starts_with("revoked 019a9b1c at "), "{stdout}");
}
