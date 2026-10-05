//! The built `efr` binary against a real daemon: `efr --help`, `efr status`, an
//! `efr send` round trip, and the model list and turn settings as `efr models`,
//! `efr settings`, `efr send` and `efr config reload` meet them, with a `TestDaemon`
//! serving in this process.
//!
//! The process gets a cleared environment that names only the test daemon's temporary
//! roots, so it never sees the real home, config or runtime directory. The daemon's
//! model is the real OpenAI provider against a local Responses server, which answers
//! whatever the prompt is.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

use std::process::Output;

use assert_cmd::Command;
use efr_protocol::{
    ConversationsList, ConversationsListResult, Method, ModelsList, ModelsListResult,
};
use efr_test_daemon::{ResponsesAnswer, ResponsesServer, TTY, TestDaemon};
use pretty_assertions::assert_eq;

/// `efr` with only the variables that point it at `daemon`.
fn efr(daemon: &TestDaemon) -> Command {
    let dirs = daemon.dirs().dirs();
    let mut command = Command::new(env!("CARGO_BIN_EXE_efr"));
    command
        .env_clear()
        .current_dir(daemon.cwd())
        .env("HOME", daemon.dirs().home())
        .env("EFR_CONFIG_DIR", dirs.config())
        .env("EFR_DATA_DIR", dirs.data())
        .env("EFR_STATE_DIR", dirs.state())
        .env("EFR_RUNTIME_DIR", dirs.runtime());
    command
}

/// Runs `command` off the runtime's workers, which must keep serving the daemon.
async fn run(mut command: Command) -> Output {
    tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn help_is_a_snapshot() {
    let output = Command::new(env!("CARGO_BIN_EXE_efr")).env_clear().arg("--help").output();
    let output = output.unwrap();

    assert!(output.status.success());
    insta::assert_snapshot!(text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reports_the_running_daemon() {
    let daemon = TestDaemon::start().await.unwrap();

    let output = run({
        let mut command = efr(&daemon);
        command.arg("status");
        command
    })
    .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(stdout.contains(&daemon.daemon_id().to_string()), "{stdout}");
    assert!(stdout.contains("vt100"), "{stdout}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_prints_the_models_answer_and_exits_when_the_turn_ends() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("Hello from **efr**."));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let context = serde_json::json!({ "pwd": daemon.cwd(), "tty": TTY }).to_string();

    let output = run({
        let mut command = efr(&daemon);
        command.args(["send", "--context-json", &context, "--", "say hello"]);
        command
    })
    .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout).trim_end(), "Hello from **efr**.", "raw markdown off a tty");
    let [request] = server.received().try_into().unwrap();
    let input = request.body["input"].to_string();
    assert!(input.contains("say hello"), "{input}");
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_starts_a_conversation_only_with_a_first_prompt() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("A fresh start."));
    let daemon = TestDaemon::builder().responses(&server).start().await.unwrap();
    let context = serde_json::json!({ "pwd": daemon.cwd(), "tty": TTY }).to_string();
    let new = |words: &[&str]| {
        let mut command = efr(&daemon);
        command.args(["new", "--context-json", &context, "--"]).args(words);
        command
    };

    let bare = run(new(&[])).await;
    assert_eq!(bare.status.code(), Some(2), "{}", text(&bare.stderr));
    let client = daemon.client().await.unwrap();
    let list = || async {
        let method = Method::ConversationsList(ConversationsList { cursor: None, limit: None });
        client.call::<ConversationsListResult>(method).await.unwrap().conversations
    };
    assert!(list().await.is_empty(), "a bare new records nothing");

    let started = run(new(&["start", "over"])).await;
    assert!(started.status.success(), "{}", text(&started.stderr));
    assert_eq!(text(&started.stdout).trim_end(), "A fresh start.");
    let conversations = list().await;
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].tty.as_deref(), Some(TTY));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_and_settings_read_the_model_list_as_the_daemon_writes_it() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let list: ModelsListResult =
        client.call(Method::ModelsList(ModelsList::default())).await.unwrap();
    drop(client);
    let efr_with = |args: &[&str]| {
        let mut command = efr(&daemon);
        command.args(args);
        command
    };

    let names = run(efr_with(&["models", "--names"])).await;
    assert!(names.status.success(), "{}", text(&names.stderr));
    let ids: Vec<&str> = list.models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(text(&names.stdout).lines().collect::<Vec<_>>(), ids);

    // A model other than the default, and the last effort it takes.
    let other = list.models.iter().find(|model| !model.default && !model.efforts.is_empty());
    let other = other.expect("the built-in list holds more than the default");
    let effort = other.efforts.last().unwrap();
    let accepted = run(efr_with(&["settings", "--model", &other.id, "--effort", effort])).await;
    assert!(accepted.status.success(), "{}", text(&accepted.stderr));
    let shown = text(&accepted.stdout);
    assert!(shown.contains(&format!("model = {}", other.id)), "{shown}");
    assert!(shown.contains(&format!("effort = {effort}")), "{shown}");

    let unknown = run(efr_with(&["settings", "--model", &other.id, "--effort", "nonesuch"])).await;
    assert_eq!(unknown.status.code(), Some(2), "{}", text(&unknown.stderr));
    assert!(text(&unknown.stderr).contains(effort.as_str()), "the choices are named");
    let model = run(efr_with(&["settings", "--model", "no-such-model"])).await;
    assert_eq!(model.status.code(), Some(2), "{}", text(&model.stderr));
    daemon.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_uses_its_prompts_settings_and_after_a_reload_the_new_defaults() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::text("First."));
    server.push(ResponsesAnswer::text("Second."));
    let daemon = TestDaemon::builder().subscription(&server).start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let list: ModelsListResult =
        client.call(Method::ModelsList(ModelsList::default())).await.unwrap();
    drop(client);
    let default = list.models.iter().find(|model| model.default).unwrap();
    let other = list.models.iter().find(|model| !model.default && model.efforts.len() > 1);
    let other = other.expect("a second model with efforts");
    let (asked, reloaded) = (&other.efforts[1], &default.efforts[0]);
    let context = serde_json::json!({ "pwd": daemon.cwd(), "tty": TTY }).to_string();
    let send = |words: &[&str]| {
        let mut command = efr(&daemon);
        command.args(["send", "--context-json", &context]).args(words);
        command
    };

    let first =
        run(send(&["--mode", "auto", "--model", &other.id, "--effort", asked, "--", "one"])).await;
    assert!(first.status.success(), "{}", text(&first.stderr));
    let note = format!("mode auto, model {}, effort {asked}", other.id);
    assert!(text(&first.stderr).contains(&note), "{}", text(&first.stderr));

    let file = daemon.dirs().dirs().config().join("config.toml");
    std::fs::write(&file, format!("[model]\neffort = \"{reloaded}\"\n")).unwrap();
    let reload = run({
        let mut command = efr(&daemon);
        command.args(["config", "reload"]);
        command
    })
    .await;
    assert!(reload.status.success(), "{}", text(&reload.stderr));
    let second = run(send(&["--", "two"])).await;
    assert!(second.status.success(), "{}", text(&second.stderr));

    let [one, two] = server.received().try_into().unwrap();
    assert_eq!(one.body["model"], other.id.as_str());
    assert_eq!(one.body["reasoning"]["effort"], asked.as_str());
    assert_eq!(two.body["model"], default.id.as_str(), "the config's model");
    assert_eq!(two.body["reasoning"]["effort"], reloaded.as_str(), "the reloaded effort");
    daemon.stop().await.unwrap();
}
