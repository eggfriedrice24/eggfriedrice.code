use std::sync::Arc;

use efr_protocol::{AdminLoginOpenAiItem, ClientFrame, ErrorBody, ErrorCode, Method};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{RecordingBrowser, TestEnv, TestInterrupt, capture, command};

const URL: &str = "https://auth.openai.com/oauth/authorize?client_id=app&state=s";

fn authorize() -> AdminLoginOpenAiItem {
    AdminLoginOpenAiItem::AuthorizeUrl { url: URL.to_owned() }
}

fn completed() -> AdminLoginOpenAiItem {
    AdminLoginOpenAiItem::Completed { provider: "openai".to_owned() }
}

#[tokio::test]
async fn login_prints_the_url_and_waits_for_the_daemon_to_finish() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let browser = Arc::new(RecordingBrowser::default());
    let ctx = Context { browser: browser.clone(), ..env.context() };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AdminLoginOpenAi(_)), "{}", method.name());
        conn.item(id, &authorize()).await;
        conn.item(id, &serde_json::json!({ "kind": "progress", "step": "exchanging" })).await;
        conn.item(id, &completed()).await;
        conn.end(id).await;
        // The stream ended on its own, so nothing is cancelled.
        assert_eq!(conn.until_closed().await, []);
    };
    let line = command(&["login", "openai"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    insta::assert_snapshot!(captured.stdout());
    assert!(browser.0.lock().unwrap().is_empty(), "no browser without EFR_OPEN_BROWSER");
}

#[tokio::test]
async fn efr_open_browser_opens_the_url() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let browser = Arc::new(RecordingBrowser::default());
    let ctx = Context {
        browser: browser.clone(),
        env: Env::fixed([(Var::OpenBrowser, "1")]),
        ..env.context()
    };
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.item(id, &authorize()).await;
        conn.item(id, &completed()).await;
        conn.end(id).await;
        conn.until_closed().await;
    };
    let line = command(&["login", "openai"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(*browser.0.lock().unwrap(), [URL]);
}

#[tokio::test]
async fn a_login_that_ends_without_completing_fails() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.item(id, &authorize()).await;
        conn.end(id).await;
        conn.until_closed().await;
    };
    let line = command(&["login", "openai"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(captured.stderr(), "efr: the daemon ended the login before it completed\n");
}

#[tokio::test]
async fn a_second_login_is_refused_as_busy() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.fail(id, ErrorBody::new(ErrorCode::Busy, "a login is already running")).await;
        conn.until_closed().await;
    };
    let line = command(&["login", "openai"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert!(captured.stderr().contains("busy: a login is already running"));
}

#[tokio::test]
async fn ctrl_c_abandons_the_login_by_closing_the_connection() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { interrupt: interrupt.clone(), ..env.context() };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.item(id, &authorize()).await;
        interrupt.trigger();
        let rest = conn.until_closed().await;
        assert!(rest.iter().all(|frame| matches!(frame, ClientFrame::Cancel { .. })));
    };
    let line = command(&["login", "openai"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Interrupted);
    assert_eq!(captured.stderr(), "");
}

#[tokio::test]
async fn an_invalid_open_browser_flag_is_reported() {
    let env = TestEnv::new();
    let ctx = Context { env: Env::fixed([(Var::OpenBrowser, "maybe")]), ..env.context() };
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["login", "openai"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::DaemonError);
    assert!(
        captured.stderr().contains("EFR_OPEN_BROWSER holds \"maybe\""),
        "{}",
        captured.stderr()
    );
}
