use efr_protocol::PushTestRow;
use pretty_assertions::assert_eq;

use super::*;
use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command};

fn row(name: &str, result: PushTestResult, code: Option<&str>) -> PushTestRow {
    PushTestRow { name: name.to_owned(), result, code: code.map(str::to_owned) }
}

fn piped() -> RenderOptions {
    RenderOptions::new(100).with_terminal(false)
}

/// TM-Commands, `efr push test`: the name, padded, then each of the texts of the spec.
#[test]
fn each_result_has_its_text() {
    let rows = vec![
        row("iPhone 17", PushTestResult::Sent, None),
        row("iPad mini", PushTestResult::NoReceipt, None),
        row("iPhone 16", PushTestResult::NotSent, Some("DeviceNotRegistered")),
        row("a", PushTestResult::NotSent, Some("InvalidCredentials")),
        row("b", PushTestResult::NotSent, Some("UNAUTHORIZED")),
        row("c", PushTestResult::NotSent, Some("MessageRateExceeded")),
        row("d", PushTestResult::NotSent, Some("TOO_MANY_REQUESTS")),
        row("e", PushTestResult::NotSent, Some("MessageTooBig")),
        row("f", PushTestResult::NotSent, None),
    ];

    let text = lines(&AdminPushTestResult { devices: rows }, &piped());

    let expected = [
        "iPhone 17  sent",
        "iPad mini  sent to Expo, no receipt yet",
        "iPhone 16  not sent: DeviceNotRegistered. Push is off for this phone now. Open the app.",
        "a          not sent: InvalidCredentials. Expo has no valid push key for this app. Run eas credentials.",
        "b          not sent: UNAUTHORIZED. The Expo account asks for an access token. efr sends none.",
        "c          not sent: MessageRateExceeded. Try again later.",
        "d          not sent: TOO_MANY_REQUESTS. Try again later.",
        "e          not sent: MessageTooBig.",
        "f          not sent: no answer from exp.host.",
    ];
    assert_eq!(text.lines().collect::<Vec<_>>(), expected);
}

#[test]
fn no_phone_with_push_is_one_muted_line_with_no_name() {
    let none = AdminPushTestResult { devices: Vec::new() };
    assert_eq!(lines(&none, &piped()), "no paired phone has push on\n");
    let painted = lines(&none, &RenderOptions::new(100));
    assert_eq!(
        painted,
        format!(
            "{}\n",
            format::paint("no paired phone has push on", Tone::Dim, &RenderOptions::new(100))
        )
    );
}

/// On a terminal the result has its role, and a line wider than the screen is one line,
/// which the terminal wraps (TM-Narrow).
#[test]
fn a_result_has_its_role_and_a_long_line_stays_one_line() {
    let options = RenderOptions::new(100);
    let rows = vec![
        row("iPhone 17", PushTestResult::Sent, None),
        row("iPad mini", PushTestResult::NoReceipt, None),
        row("old phone", PushTestResult::NotSent, Some("InvalidCredentials")),
    ];

    let text = lines(&AdminPushTestResult { devices: rows }, &options);

    let paint = |text, tone| format::paint(text, tone, &options);
    let expected = format!(
        "iPhone 17  {}\niPad mini  {}\nold phone  {}\n",
        paint("sent", Tone::Success),
        paint("sent to Expo, no receipt yet", Tone::Attention),
        paint(
            "not sent: InvalidCredentials. Expo has no valid push key for this app. Run eas \
             credentials.",
            Tone::Failure
        ),
    );
    assert_eq!(text, expected);
}

#[test]
fn a_name_never_reaches_the_terminal_raw() {
    let rows = vec![row("phone\u{1b}[31m", PushTestResult::Sent, None)];
    let text = lines(&AdminPushTestResult { devices: rows }, &piped());
    assert!(!text.contains('\u{1b}'), "{text:?}");
}

#[tokio::test]
async fn push_test_asks_the_daemon_and_prints_its_rows() {
    let env = TestEnv::new();
    let fake = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminPushTest(_)), "{}", method.name());
        let devices = vec![row("iPhone 17", PushTestResult::Sent, None)];
        conn.reply(id, &AdminPushTestResult { devices }).await;
    };
    let line = command(&["push", "test"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    assert_eq!(captured.stdout(), "iPhone 17  sent\n");
}
