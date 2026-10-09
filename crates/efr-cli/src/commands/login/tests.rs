use std::sync::Arc;

use efr_protocol::{
    AdminLoginApiKey, AdminLoginApiKeyResult, AdminLoginOpenAiItem, ClientFrame, ErrorBody,
    ErrorCode, Method,
};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::settings::Settings;
use crate::testing::{
    FixedKeyInput, RecordingBrowser, ScriptedKeys, TestEnv, TestInterrupt, capture, command,
};

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

/// A key as a test spells it; it must never show in what efr prints.
const KEY: &str = "sk-proj-efr-cli-test-key-9f3c";

/// What the daemon answers to a stored key.
fn stored(provider: &str, hint: &str, checked: bool, active: bool) -> AdminLoginApiKeyResult {
    AdminLoginApiKeyResult {
        provider: provider.to_owned(),
        key_hint: hint.to_owned(),
        checked,
        active,
    }
}

/// Runs `args` against a daemon that takes one `admin.login_api_key` and answers
/// `answer`, and returns the exit, the request and what efr printed.
async fn login_with(
    ctx: Context,
    args: &[&str],
    answer: Result<AdminLoginApiKeyResult, ErrorBody>,
    daemon: crate::testing::FakeDaemon,
) -> (Exit, AdminLoginApiKey, String, String) {
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::AdminLoginApiKey(params) = method else { panic!("{}", method.name()) };
        match answer {
            Ok(result) => conn.reply(id, &result).await,
            Err(body) => conn.fail(id, body).await,
        }
        conn.until_closed().await;
        params
    };
    let line = command(args);
    let (exit, params) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    (exit, params, captured.stdout(), captured.stderr())
}

#[tokio::test]
async fn a_key_on_stdin_is_trimmed_checked_and_shown_only_by_its_hint() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let input = FixedKeyInput { stdin: "  sk-proj-efr-cli-test-key-9f3c \n", ..Default::default() };
    let ctx = Context { key_input: Arc::new(input), ..env.context() };

    let answer = Ok(stored("openai-api", "sk-proj-...9f3c", true, false));
    let (exit, params, stdout, stderr) =
        login_with(ctx, &["login", "openai-api"], answer, daemon).await;

    assert_eq!(exit, Exit::Success);
    assert_eq!(params.provider, "openai-api");
    assert_eq!(params.key.expose_secret(), KEY);
    assert!(params.check);
    assert_eq!(stderr, "checking the key...\n");
    assert_eq!(
        stdout,
        "logged in to openai-api with key sk-proj-...9f3c\n\
         New conversations keep their provider. To use openai-api, set provider = \"openai-api\" \
         under [model] in config.toml (efr config set model.provider openai-api), then restart \
         efrd: systemctl --user restart efrd\n"
    );
    assert!(!format!("{params:?}").contains(KEY));
}

#[tokio::test]
async fn from_env_reads_the_variable_of_the_provider_and_no_check_skips_the_check() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let input = FixedKeyInput {
        vars: vec![("ANTHROPIC_API_KEY", "sk-ant-api03-efr-cli-a1b2"), ("OPENAI_API_KEY", KEY)],
        // A terminal or stdin is not read with --from-env.
        stdin: "sk-ant-wrong",
    };
    let ctx = Context { key_input: Arc::new(input), ..env.context() };

    let answer = Ok(stored("anthropic-api", "sk-ant-...a1b2", false, true));
    let (exit, params, stdout, stderr) =
        login_with(ctx, &["login", "anthropic", "--from-env", "--no-check"], answer, daemon).await;

    assert_eq!(exit, Exit::Success);
    assert_eq!(params.provider, "anthropic-api");
    assert_eq!(params.key.expose_secret(), "sk-ant-api03-efr-cli-a1b2");
    assert!(!params.check);
    assert_eq!(stderr, "", "nothing is checked");
    assert_eq!(stdout, "logged in to anthropic-api with key sk-ant-...a1b2 (not checked)\n");
}

#[tokio::test]
async fn a_variable_that_is_not_set_is_a_usage_error_before_any_connection() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (mut out, captured) = capture();

    let exit = run::run(&command(&["login", "openai-api", "--from-env"]), &ctx, &mut out).await;

    assert_eq!(exit, Exit::Usage);
    assert!(
        captured.stderr().starts_with("efr: OPENAI_API_KEY is not set in this shell\n"),
        "{}",
        captured.stderr()
    );
}

#[tokio::test]
async fn an_empty_stdin_is_a_usage_error() {
    let env = TestEnv::new();
    let input = FixedKeyInput { stdin: " \n", ..Default::default() };
    let ctx = Context { key_input: Arc::new(input), ..env.context() };
    let (mut out, captured) = capture();

    let exit = run::run(&command(&["login", "anthropic"]), &ctx, &mut out).await;

    assert_eq!(exit, Exit::Usage);
    assert!(
        captured.stderr().starts_with("efr: no key came from stdin\n"),
        "{}",
        captured.stderr()
    );
}

#[tokio::test]
async fn on_a_terminal_the_key_is_typed_at_a_prompt_that_does_not_show_it() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let typing = async {
        keys.type_bytes(b"sk-ant-api03-typed-x\x7fa1b2\r").await;
    };

    let answer = Ok(stored("anthropic-api", "sk-ant-...a1b2", true, true));
    let ((exit, params, stdout, stderr), ()) =
        tokio::join!(login_with(ctx, &["login", "anthropic"], answer, daemon), typing);

    assert_eq!(exit, Exit::Success);
    assert_eq!(params.key.expose_secret(), "sk-ant-api03-typed-a1b2", "Backspace edits the line");
    assert_eq!(stderr, "Anthropic API key (input is hidden): \nchecking the key...\n");
    assert_eq!(stdout, "logged in to anthropic-api with key sk-ant-...a1b2\n");
    assert!(keys.discarded(), "what was typed after Enter never reaches the shell");
}

#[tokio::test]
async fn a_refused_key_says_why_and_never_offers_to_skip_the_check() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let input = FixedKeyInput { stdin: KEY, ..Default::default() };
    let ctx = Context { key_input: Arc::new(input), ..env.context() };
    let body = ErrorBody::new(
        ErrorCode::Unauthorized,
        "the check of the key for openai-api failed: the provider rejected the credentials: Incorrect API key provided: sk-proj-****9f3c.",
    );

    let (exit, _, stdout, stderr) =
        login_with(ctx, &["login", "openai-api"], Err(body), daemon).await;

    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "checking the key...\nefr: the daemon failed the request with unauthorized: the check of the key for openai-api failed: the provider rejected the credentials: Incorrect API key provided: sk-proj-****9f3c.\n"
    );
    assert!(!stderr.contains(KEY));
}

#[tokio::test]
async fn a_check_without_an_answer_offers_to_store_the_key_without_it() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let input = FixedKeyInput { stdin: KEY, ..Default::default() };
    let ctx = Context { key_input: Arc::new(input), ..env.context() };
    let body = ErrorBody::new(
        ErrorCode::Internal,
        "the check of the key for openai-api failed: the request to the provider failed in transit",
    );

    let (exit, _, _, stderr) = login_with(ctx, &["login", "openai-api"], Err(body), daemon).await;

    assert_eq!(exit, Exit::DaemonError);
    assert!(
        stderr.ends_with(
            "efr: to store the key without the check: efr login openai-api --no-check\n"
        ),
        "{stderr}"
    );
}

#[tokio::test]
async fn a_key_for_the_provider_of_the_config_needs_only_a_restart() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let input = FixedKeyInput { stdin: "sk-ant-api03-efr-cli-a1b2", ..Default::default() };
    let settings = Settings::parse(
        std::path::Path::new("/c/config.toml"),
        "[model]\nprovider = \"anthropic-api\"\n",
    );
    let ctx = Context { key_input: Arc::new(input), settings, ..env.context() };

    let answer = Ok(stored("anthropic-api", "sk-ant-...a1b2", true, false));
    let (exit, _, stdout, _) = login_with(ctx, &["login", "anthropic"], answer, daemon).await;

    assert_eq!(exit, Exit::Success);
    assert_eq!(
        stdout,
        "logged in to anthropic-api with key sk-ant-...a1b2\n\
         config.toml names anthropic-api under [model], and efrd uses it after a restart: \
         systemctl --user restart efrd\n"
    );
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
